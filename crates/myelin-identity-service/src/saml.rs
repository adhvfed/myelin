//! # `saml` — REAL SAML 2.0 XML-DSig credential verification (MR-010b; the SAML slice of P-526,
//! census SI-001/004). The HARDEST credential type: SAML is the home of **XML Signature Wrapping
//! (XSW)**, the #1 SAML auth-bypass class.
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md` §4 (the authentication
//! surfaces — SAML 2.0; **tenant is taken from the verified credential, never the URL path**, ID-3).
//!
//! ## What this module replaces (the #1 CRITICAL census finding, for the SAML scheme)
//! The production auth graph's floor verifier ([`crate::authenticate::StructuralVerifier`]) parses a
//! PLAINTEXT `<tenant>|<region>|<subject_key>` envelope — so ANYONE forges any principal in any
//! tenant (SI-001/004). [`SamlVerifier`] is the REAL cryptographic replacement for the **SAML**
//! scheme: it verifies the enveloped XML-DSig signature over a SAML 2.0 assertion against the IdP's
//! INJECTED signing key, defends against the full XSW family, validates the SAML conditions, and
//! extracts a trust-rooted [`VerifiedAssertion`] — or refuses it LOUDLY. It plugs into the EXISTING
//! [`CredentialVerifier`] seam — the resolution + telemetry body in [`crate::authenticate`] does not
//! change. It is the sibling of [`crate::oidc::OidcVerifier`] / [`crate::ssh_auth::SshVerifier`] —
//! same seam, same rigor (`verify` is TOTAL over attacker bytes — no slice/`unwrap`/overflow panic).
//!
//! ## STEP-1 dependency / risk decision (hand-built exc-c14n + vetted crypto — no C deps)
//! SAML XML-DSig REQUIRES exclusive XML canonicalization (exc-c14n) of the `SignedInfo` element (to
//! verify the signature) and of the referenced element (to verify the Reference digest). There is NO
//! mature pure-Rust XML-DSig / c14n crate in `Cargo.lock`. The realistic vetted options
//! (`samael`, `xmlsec` bindings) pull **C** dependencies — `libxml2` + `openssl` — into a workspace
//! whose entire crypto stack is deliberately pure-Rust/`ring` (no openssl), and `libxml2` itself has
//! a long history of XXE / entity-expansion CVEs (precisely the attack class this prompt must
//! defend). Adding that C surface here would be a net SECURITY REGRESSION, not a gain. So the choice:
//! **hand-build a constrained exc-c14n + hand-parse the XML with the non-expanding `xmlparser`
//! tokenizer (already in `Cargo.lock` via `aws-smithy-xml` — promoting it adds NO new crate to the
//! tree), and do the signature math with the SAME vetted primitives the OIDC path uses (`rsa` +
//! `sha2` for RSA-SHA256, `ring` for ECDSA-P256-SHA256).** The XSW defence below is layered so it
//! does **not** depend on c14n subtleties (single-assertion / single-ID / reference-pinning /
//! schema-position / comment rejection are structural), giving defence-in-depth even if c14n had a
//! bug. **Honest c14n coverage (see [`canonicalize`]):** exclusive c14n WITHOUT comments, for the
//! self-contained-assertion profile — element/attribute/namespace rendering, exclusive minimal
//! namespace output (default-`xmlns` first then by prefix; the implicit `xml` prefix never emitted),
//! attribute sort by (namespace-uri, local), and canonical text/attr escaping. NOT implemented (the
//! honest gaps): the `InclusiveNamespaces` `PrefixList`, processing-instruction / comment nodes
//! (rejected outright), and DTDs/entities (rejected outright). Interop with a third-party IdP's exact
//! c14n output is therefore PROVEN here only by a real signing↔verifying round-trip (the corpus mints
//! genuine RSA/ECDSA signatures over real canonical bytes); a fixed external IdP test vector is a
//! follow-up. Inclusive (non-exclusive) c14n and SHA-1 are REJECTED.
//!
//! ## XML-DSig signature verification (the trust root is INJECTED, never the document)
//! The IdP signing key is INJECTED ([`SamlConfig::trust_anchors`]) — an attacker controls the whole
//! document, so the `<KeyInfo>` / any embedded X.509 cert is **NOT** trusted as the trust root; the
//! signature is verified ONLY against the configured anchor key(s). The `SignatureMethod` is pinned
//! to the anchor key family (an RSA anchor verifies only `rsa-sha256`; an EC anchor only
//! `ecdsa-sha256` — the alg-confusion defence). **SHA-1 (`rsa-sha1`/`dsa-sha1`/`ecdsa-sha1`) and
//! non-exclusive c14n are rejected.** Both the `SignedInfo` signature AND the Reference digest are
//! verified (a digest that does not match the canonicalized referenced element is refused).
//!
//! ## XSW DEFENCE (THE CRITICAL PART — see [`SamlVerifier::verify`])
//! After the signature checks out, claims are extracted ONLY from the exact element that was signed
//! AND referenced. The mandatory hardening (each a refusal):
//! 1. **at most ONE `<Assertion>`** in the whole document (a forged sibling/wrapped assertion → reject);
//! 2. **exactly ONE `<Signature>`**, an enveloped direct child of that one assertion (schema position);
//! 3. the Reference URI `#id` pins the signed element; there must be **exactly ONE element with that
//!    ID** in the whole document (duplicate-ID → reject) and it MUST be the single assertion;
//! 4. claims (Issuer, NameID, Conditions, Audience) are read from that signed+referenced assertion,
//!    **never** from any other element;
//! 5. **comments are rejected** anywhere in the document (the c14n-comment / `;;` NameID-injection
//!    trick relies on a comment node splitting the NameID text — no comment survives parsing);
//! 6. **DTD / DOCTYPE / entity declarations are rejected** (XXE / billion-laughs — no entity
//!    expansion; `xmlparser` is non-expanding and we additionally refuse the tokens outright).
//!
//! ## SAML conditions (all from the verified signed assertion)
//! `Issuer` must equal the configured IdP issuer; `NotBefore`/`NotOnOrAfter` are parsed to INSTANTS
//! (chrono, never a lexical compare — the MR-008 fail-open lesson) and validated with leeway;
//! `AudienceRestriction/Audience` must contain this SP; and the assertion `ID` is consumed once via
//! an injected [`crate::oidc::ReplayGuard`] (a replayed assertion is refused).
//!
//! ## What is INJECTED, and what is honestly out of scope
//! The trust anchor key(s) and the replay guard are INJECTED — the crypto/test path makes NO network
//! call. SAML metadata refresh / SP-initiated SLO / encrypted assertions (`EncryptedAssertion`) /
//! full X.509 path-building to a CA root are OUT OF SCOPE here and NOT claimed (we PIN the leaf
//! signing key, which is stricter than chain-building). These are thin later layers.
//!
//! ## Wiring (the dispatch seam — [`crate::oidc::SchemeDispatchVerifier`])
//! [`SamlVerifier`] is wired as the `saml`-scheme verifier via
//! `SchemeDispatchVerifier::route(scheme::SAML, …)` (exercised in the tests, as for OIDC/SSH). The
//! dispatcher constructs NO `Structural*` type itself (the fallback is injected by the caller), so it
//! adds no mock-crypto construction to the production graph; removing the `StructuralVerifier`
//! prod-default entirely is MR-012.

use crate::authenticate::{scheme, CredentialVerifier, VerifiedAssertion};
use crate::oidc::{JwkKey, ReplayGuard};
use myelin_identity::{AuthzError, Credential};
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

// ── The fixed SAML / XML-DSig namespace + algorithm URIs ─────────────────────────────────────────

/// The XML-DSig namespace (`<ds:Signature>` lives here).
const DS_NS: &str = "http://www.w3.org/2000/09/xmldsig#";
/// The SAML 2.0 assertion namespace (`<saml:Assertion>` lives here).
const SAML_NS: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
/// The implicit `xml` prefix namespace (RFC: never re-declared / never c14n-emitted).
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// Exclusive c14n (the ONLY canonicalization we implement / accept).
const EXC_C14N: &str = "http://www.w3.org/2001/10/xml-exc-c14n#";
/// Exclusive c14n with comments — REJECTED (comments must not be canonicalized into the signed bytes).
const EXC_C14N_COMMENTS: &str = "http://www.w3.org/2001/10/xml-exc-c14n#WithComments";
/// The enveloped-signature transform (removes the `Signature` from the referenced element).
const ENVELOPED: &str = "http://www.w3.org/2000/09/xmldsig#enveloped-signature";
/// SHA-256 digest method (the ONLY digest we accept — SHA-1 is rejected).
const SHA256_DIGEST: &str = "http://www.w3.org/2001/04/xmlenc#sha256";
/// RSA-SHA256 signature method.
const RSA_SHA256: &str = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
/// ECDSA-SHA256 signature method.
const ECDSA_SHA256: &str = "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256";

/// The maximum XML element nesting depth accepted at parse. SAML assertions are shallow; a deeply
/// nested document is a stack-exhaustion DoS against the recursive DOM traversal / canonicalization /
/// `Drop` (see the bound in [`parse_xml`]). 256 is far beyond any legitimate assertion.
const MAX_NESTING_DEPTH: usize = 256;

// ── Loud refusals ────────────────────────────────────────────────────────────────────────────────

/// A LOUD refusal of a credential that is well-formed XML but does NOT verify (bad/missing signature,
/// XSW, wrong/non-anchored key, SHA-1, tampered, expired, wrong audience, replay, comment-injection).
/// `AuthzError::FailClosed` so an unverifiable assertion NEVER resolves to a Principal.
fn refuse(msg: impl Into<String>) -> AuthzError {
    AuthzError::FailClosed(msg.into())
}

/// A LOUD structural refusal — the bytes are not even well-formed / admissible XML (garbage, a DTD,
/// an entity declaration, a comment). `AuthzError::BadRequest`.
fn malformed(msg: impl Into<String>) -> AuthzError {
    AuthzError::BadRequest(msg.into())
}

// ================================================================================================
// A minimal, hardened XML DOM — built from the non-expanding `xmlparser` tokenizer.
// DTDs / entity declarations / comments / processing instructions are REFUSED at parse time (the
// XXE / billion-laughs / comment-injection defence). `parse` is TOTAL over attacker bytes.
// ================================================================================================

/// One XML element in the hardened DOM. `ns_decls` are the namespace declarations made ON this
/// element (`("", uri)` is the default `xmlns`); `attrs` are the non-namespace attributes.
#[derive(Clone, Debug)]
struct Element {
    prefix: String,
    local: String,
    ns_decls: Vec<(String, String)>,
    attrs: Vec<Attr>,
    children: Vec<Node>,
}

/// A non-namespace attribute (`prefix` empty ⇒ no namespace; unprefixed attributes are NOT in the
/// default namespace, per XML namespaces).
#[derive(Clone, Debug)]
struct Attr {
    prefix: String,
    local: String,
    value: String,
}

/// A child node: a nested element or a run of character data (CDATA is folded into text).
#[derive(Clone, Debug)]
enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    /// The `ID` attribute value (SAML uses the unprefixed `ID` of type `xs:ID`), if present.
    fn id_attr(&self) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.prefix.is_empty() && a.local == "ID")
            .map(|a| a.value.as_str())
    }

    /// The concatenated direct text content (no comments survive parsing, so this is unambiguous —
    /// the NameID comment-injection trick cannot split this text).
    fn text(&self) -> String {
        let mut s = String::new();
        for c in &self.children {
            if let Node::Text(t) = c {
                s.push_str(t);
            }
        }
        s
    }
}

/// Parse `xml` into the hardened DOM. TOTAL over attacker bytes: malformed XML is a loud
/// `BadRequest`, never a panic. A DOCTYPE / DTD / entity declaration / comment / processing
/// instruction is REFUSED (the XXE / billion-laughs / comment-injection defence). Returns the single
/// root element.
fn parse_xml(xml: &str) -> Result<Element, AuthzError> {
    use xmlparser::{ElementEnd, Token};

    // Open elements whose start tag is complete (children append to the last); plus the element whose
    // start tag is still being read (collecting attributes).
    let mut stack: Vec<Element> = Vec::new();
    let mut pending: Option<Element> = None;
    let mut root: Option<Element> = None;

    // Attach a finished element to its parent (or record it as the root). A second root is a refusal.
    fn attach(
        el: Element,
        stack: &mut Vec<Element>,
        root: &mut Option<Element>,
    ) -> Result<(), AuthzError> {
        match stack.last_mut() {
            Some(parent) => {
                parent.children.push(Node::Element(el));
                Ok(())
            }
            None => {
                if root.is_some() {
                    return Err(malformed("XML has more than one root element"));
                }
                *root = Some(el);
                Ok(())
            }
        }
    }

    for tok in xmlparser::Tokenizer::from(xml) {
        let tok = tok.map_err(|e| malformed(format!("malformed XML: {e}")))?;
        match tok {
            // A leading `<?xml …?>` declaration is fine; ignore it.
            Token::Declaration { .. } => {}
            // XXE / billion-laughs defence — refuse any DTD / DOCTYPE / entity declaration outright.
            Token::DtdStart { .. } | Token::EmptyDtd { .. } | Token::DtdEnd { .. } => {
                return Err(malformed(
                    "DTD / DOCTYPE is rejected (XXE / entity-expansion defence — no entity processing)",
                ));
            }
            Token::EntityDeclaration { .. } => {
                return Err(malformed(
                    "entity declaration is rejected (XXE / billion-laughs defence)",
                ));
            }
            // Comments are refused (the c14n-comment / NameID-injection trick relies on a comment
            // node splitting element text; no comment ever enters the DOM).
            Token::Comment { .. } => {
                return Err(malformed(
                    "XML comments are rejected (NameID comment-injection / c14n-comment defence)",
                ));
            }
            // Processing instructions are not part of the SAML profile and are refused.
            Token::ProcessingInstruction { .. } => {
                return Err(malformed("processing instructions are rejected"));
            }
            Token::ElementStart { prefix, local, .. } => {
                if pending.is_some() {
                    // An element start before the previous one's end tag closed — malformed stream.
                    return Err(malformed("malformed XML element nesting"));
                }
                pending = Some(Element {
                    prefix: prefix.as_str().to_string(),
                    local: local.as_str().to_string(),
                    ns_decls: Vec::new(),
                    attrs: Vec::new(),
                    children: Vec::new(),
                });
            }
            Token::Attribute {
                prefix,
                local,
                value,
                ..
            } => {
                let el = pending
                    .as_mut()
                    .ok_or_else(|| malformed("XML attribute outside an element start"))?;
                let (p, l) = (prefix.as_str(), local.as_str());
                let val = unescape_xml(value.as_str())?;
                if p.is_empty() && l == "xmlns" {
                    el.ns_decls.push((String::new(), val)); // default namespace
                } else if p == "xmlns" {
                    el.ns_decls.push((l.to_string(), val)); // prefixed namespace
                } else {
                    el.attrs.push(Attr {
                        prefix: p.to_string(),
                        local: l.to_string(),
                        value: val,
                    });
                }
            }
            Token::ElementEnd { end, .. } => match end {
                ElementEnd::Open => {
                    let el = pending
                        .take()
                        .ok_or_else(|| malformed("XML element end without a start"))?;
                    // NESTING-DEPTH BOUND (DoS defence). `parse_xml` is iterative and survives any
                    // depth, but the DOWNSTREAM recursive traversals (`collect_named`/`collect_by_id`/
                    // `canonicalize`) AND the recursive `Drop` of the nested DOM overflow the stack on
                    // a WELL-FORMED (matched-close-tag) deeply-nested document — an uncatchable SIGABRT
                    // (a ~35 KB request aborts the auth process; `catch_unwind` does not save a stack
                    // overflow). Refusing here, BEFORE a too-deep DOM is ever assembled (the stack is
                    // still flat — children attach only on close), caps the recursion/Drop depth too.
                    // SAML assertions are shallow (a few dozen levels); 256 is generous.
                    if stack.len() >= MAX_NESTING_DEPTH {
                        return Err(malformed(format!(
                            "XML nesting too deep (> {MAX_NESTING_DEPTH}) — refused (a deeply-nested \
                             document is a stack-exhaustion DoS; SAML assertions are shallow)"
                        )));
                    }
                    stack.push(el);
                }
                ElementEnd::Empty => {
                    let el = pending
                        .take()
                        .ok_or_else(|| malformed("XML empty-element end without a start"))?;
                    attach(el, &mut stack, &mut root)?;
                }
                ElementEnd::Close(..) => {
                    let el = stack
                        .pop()
                        .ok_or_else(|| malformed("XML close tag with no open element"))?;
                    attach(el, &mut stack, &mut root)?;
                }
            },
            Token::Text { text } => {
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::Text(unescape_xml(text.as_str())?));
                }
                // Text outside the root element is whitespace-only in practice; ignore it.
            }
            Token::Cdata { text, .. } => {
                if let Some(parent) = stack.last_mut() {
                    // CDATA content is literal character data; store as text (escaped on c14n output).
                    parent.children.push(Node::Text(text.as_str().to_string()));
                }
            }
        }
    }

    if pending.is_some() || !stack.is_empty() {
        return Err(malformed("truncated XML (unclosed element)"));
    }
    root.ok_or_else(|| malformed("empty XML (no root element)"))
}

/// Unescape the five predefined XML entities + numeric character references in a raw span. ANY other
/// `&name;` reference is REFUSED (defence-in-depth vs entity smuggling, even though DTDs are already
/// rejected). TOTAL over attacker bytes (no panic on a dangling `&` or a bad numeric ref).
fn unescape_xml(raw: &str) -> Result<String, AuthzError> {
    if !raw.contains('&') {
        return Ok(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let semi = after
            .find(';')
            .ok_or_else(|| malformed("XML: unterminated entity reference (dangling `&`)"))?;
        let ent = &after[..semi];
        match ent {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                let cp = u32::from_str_radix(&ent[2..], 16)
                    .map_err(|_| malformed("XML: bad hex character reference"))?;
                out.push(
                    char::from_u32(cp)
                        .ok_or_else(|| malformed("XML: invalid character reference"))?,
                );
            }
            _ if ent.starts_with('#') => {
                let cp = ent[1..]
                    .parse::<u32>()
                    .map_err(|_| malformed("XML: bad decimal character reference"))?;
                out.push(
                    char::from_u32(cp)
                        .ok_or_else(|| malformed("XML: invalid character reference"))?,
                );
            }
            other => {
                return Err(malformed(format!(
                    "XML: unknown entity reference `&{other};` (only the predefined entities + \
                     numeric refs are allowed — no entity expansion)"
                )));
            }
        }
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

// ================================================================================================
// Exclusive XML canonicalization (exc-c14n, WITHOUT comments) — the hand-built subset.
// ================================================================================================

/// The in-scope namespace bindings (prefix → uri; `""` is the default namespace). The implicit `xml`
/// prefix is pre-bound and never emitted.
type NsScope = BTreeMap<String, String>;

/// A fresh resolution scope with the implicit `xml` prefix bound (RFC: always in scope).
fn base_scope() -> NsScope {
    let mut s = NsScope::new();
    s.insert("xml".to_string(), XML_NS.to_string());
    s
}

/// Resolve a prefix to its namespace URI in `scope` (`""` ⇒ the default namespace, or no namespace).
fn resolve<'a>(scope: &'a NsScope, prefix: &str) -> &'a str {
    scope.get(prefix).map(|s| s.as_str()).unwrap_or("")
}

/// Push a qualified name (`prefix:local`, or just `local`).
fn push_qname(out: &mut String, prefix: &str, local: &str) {
    if !prefix.is_empty() {
        out.push_str(prefix);
        out.push(':');
    }
    out.push_str(local);
}

/// Canonical attribute-value escaping (Canonical XML 1.0): `&` `<` `"` and TAB/LF/CR.
fn push_attr_value(out: &mut String, v: &str) {
    for c in v.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#x9;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            _ => out.push(c),
        }
    }
}

/// Canonical text-node escaping (Canonical XML 1.0): `&` `<` `>` and CR.
fn push_text(out: &mut String, t: &str) {
    for c in t.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#xD;"),
            _ => out.push(c),
        }
    }
}

/// **Exclusive XML canonicalization (exc-c14n) of an element subtree, WITHOUT comments.** `inherited`
/// is the in-scope namespace context from the element's ancestors (for prefix RESOLUTION); `rendered`
/// is the set of namespace declarations already emitted by output ancestors (so the EXCLUSIVE rule
/// emits a namespace only where it is visibly utilized and not already output with the same value).
/// If `skip_signature` is set, any `ds:Signature` descendant subtree is omitted — this realises the
/// enveloped-signature transform when digesting the referenced assertion.
///
/// Coverage (the honest subset): element + attribute + namespace rendering; default-`xmlns` emitted
/// before prefixed declarations, prefixed declarations sorted by prefix; the implicit `xml` prefix
/// never emitted; attributes sorted by (namespace-uri, local); canonical escaping. NOT implemented:
/// the `InclusiveNamespaces` `PrefixList` (we render the exclusive default — empty prefix list),
/// comments / PIs (rejected at parse).
fn canonicalize(
    el: &Element,
    inherited: &NsScope,
    rendered: &NsScope,
    skip_signature: bool,
    out: &mut String,
) {
    // The resolution scope at this element = inherited + this element's own declarations.
    let mut scope = inherited.clone();
    for (p, u) in &el.ns_decls {
        scope.insert(p.clone(), u.clone());
    }

    // The namespaces VISIBLY UTILIZED by this element: the element's own prefix, plus each prefixed
    // attribute's prefix. (Unprefixed attributes are in no namespace and do not utilise the default.)
    let mut utilized: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    utilized.insert(el.prefix.clone());
    for a in &el.attrs {
        if !a.prefix.is_empty() {
            utilized.insert(a.prefix.clone());
        }
    }

    // The exclusive set of namespace declarations to EMIT on this element.
    let mut to_render: Vec<(String, String)> = Vec::new();
    for p in &utilized {
        if p == "xml" {
            continue; // the implicit xml prefix is never emitted.
        }
        let uri = resolve(&scope, p).to_string();
        if p.is_empty() && uri.is_empty() {
            // The element is in no namespace: emit `xmlns=""` ONLY to override a non-empty default
            // that an output ancestor already rendered.
            if rendered.get("").is_some_and(|u| !u.is_empty()) {
                to_render.push((String::new(), String::new()));
            }
            continue;
        }
        match rendered.get(p) {
            Some(r) if r == &uri => {} // already rendered with the same value — exclusive: skip.
            _ => to_render.push((p.clone(), uri)),
        }
    }
    // Namespace declarations: default ("") sorts first, then by prefix (BTree/`sort` gives this).
    to_render.sort_by(|a, b| a.0.cmp(&b.0));

    // The rendered context passed to children = ancestors' rendered + what we emit here.
    let mut child_rendered = rendered.clone();
    for (p, u) in &to_render {
        child_rendered.insert(p.clone(), u.clone());
    }

    // Attributes sorted by (namespace-uri, local-name); unprefixed ⇒ empty uri (sorts first).
    let mut attrs: Vec<&Attr> = el.attrs.iter().collect();
    attrs.sort_by(|a, b| {
        let ua = if a.prefix.is_empty() {
            ""
        } else {
            resolve(&scope, &a.prefix)
        };
        let ub = if b.prefix.is_empty() {
            ""
        } else {
            resolve(&scope, &b.prefix)
        };
        ua.cmp(ub).then_with(|| a.local.cmp(&b.local))
    });

    // Open tag.
    out.push('<');
    push_qname(out, &el.prefix, &el.local);
    for (p, u) in &to_render {
        if p.is_empty() {
            out.push_str(" xmlns=\"");
        } else {
            out.push_str(" xmlns:");
            out.push_str(p);
            out.push_str("=\"");
        }
        push_attr_value(out, u);
        out.push('"');
    }
    for a in &attrs {
        out.push(' ');
        push_qname(out, &a.prefix, &a.local);
        out.push_str("=\"");
        push_attr_value(out, &a.value);
        out.push('"');
    }
    out.push('>');

    // Children, in document order.
    for child in &el.children {
        match child {
            Node::Text(t) => push_text(out, t),
            Node::Element(c) => {
                if skip_signature && c.local == "Signature" && child_ns_matches(c, &scope) == DS_NS {
                    continue; // the enveloped-signature transform removes the Signature subtree.
                }
                canonicalize(c, &scope, &child_rendered, skip_signature, out);
            }
        }
    }

    // Close tag.
    out.push_str("</");
    push_qname(out, &el.prefix, &el.local);
    out.push('>');
}

// ================================================================================================
// DOM navigation (namespace-aware), carrying the inherited scope for c14n.
// ================================================================================================

/// Recursively collect every element matching `(ns, local)`, paired with the INHERITED scope at that
/// element (the scope WITHOUT the element's own declarations — what [`canonicalize`] expects).
fn collect_named<'a>(
    el: &'a Element,
    inherited: &NsScope,
    ns: &str,
    local: &str,
    out: &mut Vec<(&'a Element, NsScope)>,
) {
    let mut scope = inherited.clone();
    for (p, u) in &el.ns_decls {
        scope.insert(p.clone(), u.clone());
    }
    if resolve(&scope, &el.prefix) == ns && el.local == local {
        out.push((el, inherited.clone()));
    }
    for c in &el.children {
        if let Node::Element(c) = c {
            collect_named(c, &scope, ns, local, out);
        }
    }
}

/// Recursively collect every element with an `ID` attribute equal to `id` (across the whole document
/// — the duplicate-ID / XSW defence reads this).
fn collect_by_id<'a>(el: &'a Element, id: &str, out: &mut Vec<&'a Element>) {
    if el.id_attr() == Some(id) {
        out.push(el);
    }
    for c in &el.children {
        if let Node::Element(c) = c {
            collect_by_id(c, id, out);
        }
    }
}

/// Direct child elements (namespace-aware match), with the scope resolved at the parent so the caller
/// can resolve the children's own names.
fn child_named<'a>(parent: &'a Element, scope: &NsScope, ns: &str, local: &str) -> Option<&'a Element> {
    let mut s = scope.clone();
    for (p, u) in &parent.ns_decls {
        s.insert(p.clone(), u.clone());
    }
    parent.children.iter().find_map(|c| match c {
        Node::Element(e) if child_ns_matches(e, &s) == ns && e.local == local => Some(e),
        _ => None,
    })
}

/// Resolve a child element's own namespace URI, accounting for namespace declarations made ON the
/// child itself (e.g. `<ds:Signature xmlns:ds="…">` declares its own prefix). `parent_scope` is the
/// scope in effect inside the parent.
fn child_ns_matches(child: &Element, parent_scope: &NsScope) -> String {
    if child.ns_decls.is_empty() {
        return resolve(parent_scope, &child.prefix).to_string();
    }
    let mut s = parent_scope.clone();
    for (p, u) in &child.ns_decls {
        s.insert(p.clone(), u.clone());
    }
    resolve(&s, &child.prefix).to_string()
}

/// All direct child elements matching `(ns, local)`.
fn children_named<'a>(
    parent: &'a Element,
    scope: &NsScope,
    ns: &str,
    local: &str,
) -> Vec<&'a Element> {
    let mut s = scope.clone();
    for (p, u) in &parent.ns_decls {
        s.insert(p.clone(), u.clone());
    }
    parent
        .children
        .iter()
        .filter_map(|c| match c {
            Node::Element(e) if child_ns_matches(e, &s) == ns && e.local == local => Some(e),
            _ => None,
        })
        .collect()
}

/// Read the `Algorithm` attribute of an element (the c14n/transform/digest/signature method id).
fn algorithm(el: &Element) -> Option<&str> {
    el.attrs
        .iter()
        .find(|a| a.prefix.is_empty() && a.local == "Algorithm")
        .map(|a| a.value.as_str())
}

// ================================================================================================
// Configuration + clock + trust anchor.
// ================================================================================================

/// The "now" source, in Unix seconds — injected so a test pins the clock across the
/// `NotBefore`/`NotOnOrAfter` boundary (the production default reads the system clock).
type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The SP (relying-party) configuration the verifier validates a SAML assertion against: the IdP
/// issuer it trusts, the audience (SP entity id) it IS, the INJECTED IdP signing key(s) (the trust
/// root — the document's `KeyInfo` is never trusted), the SAML attribute names the tenant/region are
/// read from, the fallback region, and the clock-skew leeway.
#[derive(Clone)]
pub struct SamlConfig {
    /// The exact `<saml:Issuer>` the assertion must carry (the configured IdP entity id).
    pub issuer: String,
    /// The audience this SP IS — `<saml:Audience>` MUST contain it.
    pub sp_entity_id: String,
    /// The INJECTED IdP signing public key(s) — the trust root. The signature must verify against one
    /// of these; the document's embedded cert / `KeyInfo` is NEVER trusted. Multiple keys support
    /// rotation.
    pub trust_anchors: Vec<JwkKey>,
    /// The SAML `<Attribute Name=…>` the TENANT is read from (default `tenant`).
    pub tenant_attr: String,
    /// The SAML `<Attribute Name=…>` the REGION is read from (default `region`).
    pub region_attr: String,
    /// The region to use if the assertion carries no region attribute (the configured IdP-binding
    /// region). If `None` and no region attribute is present, the assertion is refused.
    pub region_default: Option<String>,
    /// Clock-skew leeway, in seconds, applied to `NotBefore`/`NotOnOrAfter` (default 60).
    pub leeway_secs: i64,
}

impl SamlConfig {
    /// A config for `issuer` / `sp_entity_id` with the conventional defaults (`tenant`/`region`
    /// attributes, no fallback region, 60s leeway) and the injected `trust_anchors`.
    pub fn new(
        issuer: impl Into<String>,
        sp_entity_id: impl Into<String>,
        trust_anchors: Vec<JwkKey>,
    ) -> SamlConfig {
        SamlConfig {
            issuer: issuer.into(),
            sp_entity_id: sp_entity_id.into(),
            trust_anchors,
            tenant_attr: "tenant".into(),
            region_attr: "region".into(),
            region_default: None,
            leeway_secs: 60,
        }
    }

    /// Override the tenant/region attribute names (builder form).
    pub fn with_attrs(mut self, tenant_attr: impl Into<String>, region_attr: impl Into<String>) -> SamlConfig {
        self.tenant_attr = tenant_attr.into();
        self.region_attr = region_attr.into();
        self
    }

    /// Set the fallback region (the configured IdP-binding region) used when the assertion carries no
    /// region attribute (builder form).
    pub fn with_region_default(mut self, region: impl Into<String>) -> SamlConfig {
        self.region_default = Some(region.into());
        self
    }
}

/// The signature method (pinned to SHA-256; SHA-1 is rejected at parse of the `SignatureMethod`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SigMethod {
    RsaSha256,
    EcdsaSha256,
}

// ================================================================================================
// Signature math — vetted primitives only (the SAME ones the OIDC path uses). No hand-rolled crypto.
// ================================================================================================

/// Verify a SAML XML-DSig signature over `signed_info_c14n` with the INJECTED trust-anchor key, using
/// the vetted crate for the key family. The `SignatureMethod` MUST match the anchor key family (an
/// RSA anchor verifies only `rsa-sha256`; an EC anchor only `ecdsa-sha256` — the alg-confusion
/// defence). Ed25519 anchors are not part of XML-DSig here and are refused. NO signature math is
/// hand-rolled.
fn verify_xmldsig(
    key: &JwkKey,
    method: SigMethod,
    signed_info_c14n: &[u8],
    sig: &[u8],
) -> Result<(), AuthzError> {
    match (key, method) {
        (JwkKey::Rsa { n, e }, SigMethod::RsaSha256) => {
            use rsa::pkcs1v15::{Signature, VerifyingKey};
            use rsa::signature::Verifier;
            use rsa::{BigUint, RsaPublicKey};
            use sha2::Sha256;
            let pubkey = RsaPublicKey::new(BigUint::from_bytes_be(n), BigUint::from_bytes_be(e))
                .map_err(|e| refuse(format!("invalid RSA trust-anchor key: {e}")))?;
            let vk = VerifyingKey::<Sha256>::new(pubkey);
            let signature =
                Signature::try_from(sig).map_err(|_| refuse("malformed rsa-sha256 signature"))?;
            vk.verify(signed_info_c14n, &signature)
                .map_err(|_| refuse("XML-DSig rsa-sha256 signature verification failed"))
        }
        (JwkKey::EcP256 { x, y }, SigMethod::EcdsaSha256) => {
            use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
            if x.len() != 32 || y.len() != 32 {
                return Err(refuse("invalid P-256 trust-anchor coordinates (expected 32 bytes each)"));
            }
            // XML-DSig ECDSA SignatureValue is the IEEE-P1363 fixed `r‖s` (RFC 4051) — exactly what
            // ring's *_FIXED verifier expects. The SEC1 uncompressed point is `0x04 ‖ x ‖ y`.
            let mut point = Vec::with_capacity(65);
            point.push(0x04);
            point.extend_from_slice(x);
            point.extend_from_slice(y);
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
                .verify(signed_info_c14n, sig)
                .map_err(|_| refuse("XML-DSig ecdsa-sha256 signature verification failed"))
        }
        // The SignatureMethod does not match the anchor key family (the alg-confusion defence), or an
        // unsupported anchor family (Ed25519) was configured.
        _ => Err(refuse(
            "signature method does not match the configured trust-anchor key family \
             (alg-confusion defence)",
        )),
    }
}

/// Decode a base64 blob that may contain XML whitespace (newlines/indentation around DigestValue /
/// SignatureValue). Whitespace is stripped before decoding. Malformed base64 is a loud refusal.
fn b64_decode_ws(s: &str) -> Result<Vec<u8>, AuthzError> {
    let compact: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    B64.decode(compact.as_bytes())
        .map_err(|e| malformed(format!("malformed base64 in signature/digest: {e}")))
}

// ================================================================================================
// The verifier.
// ================================================================================================

/// **The REAL SAML 2.0 XML-DSig credential verifier (MR-010b).** Verifies the enveloped signature on
/// a SAML assertion against the INJECTED IdP key, defends against the XSW family, validates the SAML
/// conditions, and resolves the tenant/region/subject from the signed+referenced assertion — or
/// refuses loudly. `verify` is TOTAL over attacker bytes. Plugs into the existing
/// [`CredentialVerifier`] seam; the [`crate::authenticate`] resolution + telemetry body is unchanged.
#[derive(Clone)]
pub struct SamlVerifier {
    config: SamlConfig,
    replay: ReplayGuard,
    now: NowFn,
}

impl SamlVerifier {
    /// Build the verifier over an SP config (with the injected trust anchors), a fresh replay guard,
    /// and the system clock. Wire it as the `saml`-scheme verifier via
    /// `SchemeDispatchVerifier::route(scheme::SAML, …)`.
    pub fn new(config: SamlConfig) -> SamlVerifier {
        SamlVerifier {
            config,
            replay: ReplayGuard::new(),
            now: Arc::new(system_now),
        }
    }

    /// Build over an EXPLICIT shared [`ReplayGuard`] (so several verifier handles share one seen-set).
    pub fn with_replay_guard(mut self, replay: ReplayGuard) -> SamlVerifier {
        self.replay = replay;
        self
    }

    /// Build with an injected clock (Unix seconds) — the deterministic-test / drill seam.
    pub fn with_clock(mut self, now: impl Fn() -> i64 + Send + Sync + 'static) -> SamlVerifier {
        self.now = Arc::new(now);
        self
    }

    /// The shared replay guard (so a caller can pre-seed / inspect the seen-set).
    pub fn replay_guard(&self) -> &ReplayGuard {
        &self.replay
    }

    fn now(&self) -> i64 {
        (self.now)()
    }

    /// Parse a SAML `xs:dateTime` (`NotBefore`/`NotOnOrAfter`) to a Unix-second INSTANT. Parsed with
    /// chrono and compared by instant — NEVER lexically (the MR-008 fail-open lesson). A non-parseable
    /// timestamp is a loud refusal (fail-CLOSED), never coerced.
    fn instant(value: &str) -> Result<i64, AuthzError> {
        chrono::DateTime::parse_from_rfc3339(value.trim())
            .map(|dt| dt.timestamp())
            .map_err(|e| refuse(format!("unparseable SAML dateTime `{value}`: {e}")))
    }
}

impl CredentialVerifier for SamlVerifier {
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion> {
        // This verifier owns ONLY the saml scheme; another scheme is a wiring error.
        if credential.scheme != scheme::SAML {
            return Err(malformed(format!(
                "SamlVerifier received a `{}` credential (expected `saml`)",
                credential.scheme
            )));
        }

        // (1) PARSE — TOTAL over attacker bytes; DTD/entity/comment/PI refused (XXE / comment-inj).
        let root = parse_xml(credential.material.trim())?;
        let base = base_scope();

        // (2) XSW — at most ONE Assertion in the whole document. A forged sibling/wrapped/relocated
        //     assertion (the classic XSW shapes) means >1 here → refuse before any crypto.
        let mut assertions: Vec<(&Element, NsScope)> = Vec::new();
        collect_named(&root, &base, SAML_NS, "Assertion", &mut assertions);
        if assertions.is_empty() {
            return Err(refuse("no <saml:Assertion> in the document"));
        }
        if assertions.len() > 1 {
            return Err(refuse(format!(
                "XSW defence: {} <saml:Assertion> elements present — exactly one is required (a \
                 wrapped/forged assertion was injected)",
                assertions.len()
            )));
        }
        let (assertion, assertion_inherited) = assertions.remove(0);

        // (3) XSW — exactly ONE Signature in the whole document, and it MUST be the enveloped direct
        //     child of the one assertion (schema position). A Signature anywhere else (or a second
        //     one) is an XSW shape.
        let mut signatures: Vec<(&Element, NsScope)> = Vec::new();
        collect_named(&root, &base, DS_NS, "Signature", &mut signatures);
        if signatures.len() != 1 {
            return Err(refuse(format!(
                "XSW defence: expected exactly one <ds:Signature>, found {}",
                signatures.len()
            )));
        }
        let (signature, sig_inherited) = signatures.remove(0);
        // The signature must be a DIRECT child of the one assertion (enveloped position).
        let sig_is_enveloped_child =
            child_named(assertion, &assertion_inherited, DS_NS, "Signature").is_some_and(|c| {
                std::ptr::eq(c as *const Element, signature as *const Element)
            });
        if !sig_is_enveloped_child {
            return Err(refuse(
                "XSW defence: the <ds:Signature> is not the enveloped direct child of the signed \
                 assertion (schema-position violation)",
            ));
        }

        // (4) Parse SignedInfo: c14n method (must be exclusive, non-comment), signature method (pinned
        //     SHA-256 — SHA-1 refused), and exactly one Reference.
        let signed_info = child_named(signature, &sig_inherited, DS_NS, "SignedInfo")
            .ok_or_else(|| refuse("Signature has no SignedInfo"))?;
        let signed_info_scope = {
            let mut s = sig_inherited.clone();
            for (p, u) in &signature.ns_decls {
                s.insert(p.clone(), u.clone());
            }
            s
        };

        let c14n_method = child_named(signed_info, &signed_info_scope, DS_NS, "CanonicalizationMethod")
            .and_then(algorithm)
            .ok_or_else(|| refuse("SignedInfo has no CanonicalizationMethod"))?;
        match c14n_method {
            EXC_C14N => {}
            EXC_C14N_COMMENTS => {
                return Err(refuse(
                    "exclusive-c14n WITH comments is rejected (comments must not be signed)",
                ))
            }
            other => {
                return Err(refuse(format!(
                    "unsupported CanonicalizationMethod `{other}` (only exclusive c14n is accepted)"
                )))
            }
        }

        let sig_alg = child_named(signed_info, &signed_info_scope, DS_NS, "SignatureMethod")
            .and_then(algorithm)
            .ok_or_else(|| refuse("SignedInfo has no SignatureMethod"))?;
        let method = match sig_alg {
            RSA_SHA256 => SigMethod::RsaSha256,
            ECDSA_SHA256 => SigMethod::EcdsaSha256,
            other => {
                return Err(refuse(format!(
                    "unsupported/weak SignatureMethod `{other}` (only rsa-sha256 / ecdsa-sha256 are \
                     accepted — SHA-1 is rejected)"
                )))
            }
        };

        let references = children_named(signed_info, &signed_info_scope, DS_NS, "Reference");
        if references.len() != 1 {
            return Err(refuse(format!(
                "expected exactly one <ds:Reference>, found {}",
                references.len()
            )));
        }
        let reference = references[0];
        let reference_scope = {
            let mut s = signed_info_scope.clone();
            for (p, u) in &signed_info.ns_decls {
                s.insert(p.clone(), u.clone());
            }
            s
        };

        // (5) XSW — the Reference URI `#id` pins the signed element. Require a same-document fragment
        //     reference (an empty/whole-doc or detached/external reference is refused), exactly one
        //     element with that ID in the WHOLE document (duplicate-ID → refuse), and that element
        //     MUST be the single assertion we read claims from.
        let uri = reference
            .attrs
            .iter()
            .find(|a| a.prefix.is_empty() && a.local == "URI")
            .map(|a| a.value.as_str())
            .ok_or_else(|| refuse("Reference has no URI (a detached reference is refused)"))?;
        let ref_id = uri.strip_prefix('#').ok_or_else(|| {
            refuse(format!(
                "Reference URI `{uri}` is not a same-document `#id` fragment (detached/whole-document \
                 references are refused — XSW defence)"
            ))
        })?;
        if ref_id.is_empty() {
            return Err(refuse("Reference URI `#` is empty"));
        }
        let mut by_id: Vec<&Element> = Vec::new();
        collect_by_id(&root, ref_id, &mut by_id);
        if by_id.len() != 1 {
            return Err(refuse(format!(
                "XSW defence: {} elements carry ID `{ref_id}` — the signed-element reference must be \
                 unique (duplicate-ID wrapping)",
                by_id.len()
            )));
        }
        if !std::ptr::eq(by_id[0] as *const Element, assertion as *const Element) {
            return Err(refuse(
                "XSW defence: the signed Reference does not point at the assertion the claims are \
                 read from (a wrapped/relocated assertion)",
            ));
        }

        // (6) Transforms — must be exactly {enveloped-signature, exclusive-c14n}; any other transform
        //     (XPath / XSLT / non-exclusive c14n) is refused (a permissive transform set is an XSW /
        //     digest-bypass vector).
        let transforms = child_named(reference, &reference_scope, DS_NS, "Transforms");
        let transform_algs: Vec<&str> = transforms
            .map(|t| {
                let ts = {
                    let mut s = reference_scope.clone();
                    for (p, u) in &reference.ns_decls {
                        s.insert(p.clone(), u.clone());
                    }
                    s
                };
                children_named(t, &ts, DS_NS, "Transform")
                    .into_iter()
                    .filter_map(algorithm)
                    .collect()
            })
            .unwrap_or_default();
        if !transform_algs.contains(&ENVELOPED) {
            return Err(refuse(
                "Reference is missing the enveloped-signature transform",
            ));
        }
        if !transform_algs.contains(&EXC_C14N) {
            return Err(refuse("Reference is missing the exclusive-c14n transform"));
        }
        for t in &transform_algs {
            if *t != ENVELOPED && *t != EXC_C14N {
                return Err(refuse(format!(
                    "unsupported/dangerous Reference transform `{t}` (only enveloped-signature + \
                     exclusive-c14n are accepted)"
                )));
            }
        }

        // (7) DigestMethod must be SHA-256 (SHA-1 refused). Verify the Reference digest: canonicalize
        //     the referenced assertion with the Signature removed (the enveloped transform), SHA-256
        //     it, and compare to DigestValue.
        let digest_method = child_named(reference, &reference_scope, DS_NS, "DigestMethod")
            .and_then(algorithm)
            .ok_or_else(|| refuse("Reference has no DigestMethod"))?;
        if digest_method != SHA256_DIGEST {
            return Err(refuse(format!(
                "unsupported/weak DigestMethod `{digest_method}` (only sha256 is accepted)"
            )));
        }
        let digest_value = child_named(reference, &reference_scope, DS_NS, "DigestValue")
            .map(|d| d.text())
            .ok_or_else(|| refuse("Reference has no DigestValue"))?;
        let expected_digest = b64_decode_ws(&digest_value)?;

        let mut ref_c14n = String::new();
        canonicalize(
            assertion,
            &assertion_inherited,
            &NsScope::new(),
            true, // enveloped-signature transform: drop the Signature subtree.
            &mut ref_c14n,
        );
        let actual_digest = {
            use sha2::{Digest, Sha256};
            Sha256::digest(ref_c14n.as_bytes()).to_vec()
        };
        if actual_digest != expected_digest {
            return Err(refuse(
                "Reference digest mismatch: the canonicalized signed assertion does not match the \
                 DigestValue (tampered assertion, or XSW)",
            ));
        }

        // (8) SIGNATURE — canonicalize SignedInfo (exclusive c14n) and verify the SignatureValue
        //     against the INJECTED trust anchor(s). The document's KeyInfo is NEVER consulted.
        let mut signed_info_c14n = String::new();
        canonicalize(
            signed_info,
            &signed_info_scope,
            &NsScope::new(),
            false,
            &mut signed_info_c14n,
        );
        let signature_value = child_named(signature, &sig_inherited, DS_NS, "SignatureValue")
            .map(|s| s.text())
            .ok_or_else(|| refuse("Signature has no SignatureValue"))?;
        let sig_bytes = b64_decode_ws(&signature_value)?;

        if self.config.trust_anchors.is_empty() {
            return Err(refuse(
                "no IdP trust anchor configured — cannot verify the SAML signature (fail-closed)",
            ));
        }
        let mut verified = false;
        let mut last_err: Option<AuthzError> = None;
        for anchor in &self.config.trust_anchors {
            match verify_xmldsig(anchor, method, signed_info_c14n.as_bytes(), &sig_bytes) {
                Ok(()) => {
                    verified = true;
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        if !verified {
            return Err(last_err.unwrap_or_else(|| {
                refuse("SAML signature did not verify against any configured trust anchor")
            }));
        }

        // ── The signature + digest are proven; from here every fact comes from the SIGNED assertion ──

        // (9) Issuer — must equal the configured IdP issuer.
        let issuer = child_named(assertion, &assertion_inherited, SAML_NS, "Issuer")
            .map(|e| e.text())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| refuse("signed assertion has no <saml:Issuer>"))?;
        if issuer != self.config.issuer {
            return Err(refuse(format!(
                "issuer mismatch: assertion Issuer `{issuer}` != configured `{}`",
                self.config.issuer
            )));
        }

        // (10) Conditions — NotBefore/NotOnOrAfter parsed to INSTANTS (with leeway); AudienceRestriction
        //      must list this SP. (Schema position: Conditions is a direct child of the assertion.)
        let now = self.now();
        let leeway = self.config.leeway_secs;
        if let Some(conditions) = child_named(assertion, &assertion_inherited, SAML_NS, "Conditions") {
            let cond_scope = {
                let mut s = assertion_inherited.clone();
                for (p, u) in &assertion.ns_decls {
                    s.insert(p.clone(), u.clone());
                }
                s
            };
            if let Some(nb) = conditions
                .attrs
                .iter()
                .find(|a| a.prefix.is_empty() && a.local == "NotBefore")
            {
                let nb = SamlVerifier::instant(&nb.value)?;
                if nb.saturating_sub(leeway) > now {
                    return Err(refuse(format!(
                        "assertion not yet valid: NotBefore instant {nb} (-{leeway}s) > now {now}"
                    )));
                }
            }
            if let Some(na) = conditions
                .attrs
                .iter()
                .find(|a| a.prefix.is_empty() && a.local == "NotOnOrAfter")
            {
                let na = SamlVerifier::instant(&na.value)?;
                if na.saturating_add(leeway) <= now {
                    return Err(refuse(format!(
                        "assertion expired: NotOnOrAfter instant {na} (+{leeway}s) <= now {now}"
                    )));
                }
            }
            // AudienceRestriction/Audience — at least one Audience must equal this SP.
            let mut audiences: Vec<String> = Vec::new();
            for ar in children_named(conditions, &cond_scope, SAML_NS, "AudienceRestriction") {
                let ar_scope = {
                    let mut s = cond_scope.clone();
                    for (p, u) in &conditions.ns_decls {
                        s.insert(p.clone(), u.clone());
                    }
                    s
                };
                for a in children_named(ar, &ar_scope, SAML_NS, "Audience") {
                    audiences.push(a.text().trim().to_string());
                }
            }
            if !audiences.is_empty() && !audiences.iter().any(|a| a == &self.config.sp_entity_id) {
                return Err(refuse(format!(
                    "audience mismatch: assertion AudienceRestriction does not contain this SP `{}`",
                    self.config.sp_entity_id
                )));
            }
            if audiences.is_empty() {
                return Err(refuse(
                    "assertion Conditions has no AudienceRestriction/Audience (cannot confirm this \
                     assertion was issued for this SP)",
                ));
            }
        } else {
            return Err(refuse(
                "signed assertion has no <saml:Conditions> (cannot validate validity window / audience)",
            ));
        }

        // (11) Subject / NameID — the subject key. Schema position: Subject is a direct child of the
        //      assertion, NameID a direct child of Subject. (No comment can split the NameID text —
        //      comments were rejected at parse.)
        let subject = child_named(assertion, &assertion_inherited, SAML_NS, "Subject")
            .ok_or_else(|| refuse("signed assertion has no <saml:Subject>"))?;
        let subject_scope = {
            let mut s = assertion_inherited.clone();
            for (p, u) in &assertion.ns_decls {
                s.insert(p.clone(), u.clone());
            }
            s
        };
        let name_id = child_named(subject, &subject_scope, SAML_NS, "NameID")
            .map(|e| e.text())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| refuse("signed assertion Subject has no (non-empty) <saml:NameID>"))?;

        // (12) Tenant / region — from the SIGNED assertion's AttributeStatement (the trust root, ID-3),
        //      never from any caller path / wrapper. Region falls back to the configured IdP-binding
        //      region if the assertion carries none.
        let attrs = saml_attributes(assertion, &assertion_inherited);
        let tenant = attrs
            .get(&self.config.tenant_attr)
            .filter(|v| !v.is_empty())
            .cloned()
            .ok_or_else(|| {
                refuse(format!(
                    "signed assertion carries no `{}` attribute (the tenant is the trust root and \
                     must come from the IdP-signed assertion, never a path)",
                    self.config.tenant_attr
                ))
            })?;
        let region = attrs
            .get(&self.config.region_attr)
            .filter(|v| !v.is_empty())
            .cloned()
            .or_else(|| self.config.region_default.clone())
            .ok_or_else(|| {
                refuse(format!(
                    "signed assertion carries no `{}` attribute and no fallback region is configured",
                    self.config.region_attr
                ))
            })?;

        // (13) REPLAY — consume the assertion ID once. A replayed assertion (same ID) is refused. Done
        //      LAST so a refused (invalid) assertion does not burn the ID.
        if !self.replay.consume(ref_id) {
            return Err(refuse(format!(
                "replayed SAML assertion: ID `{ref_id}` was already presented (replay defence)"
            )));
        }

        // (14) THE TRUST-ROOTED ASSERTION — tenant/region/subject from the verified SIGNED assertion.
        Ok(VerifiedAssertion {
            tenant: TenantId(tenant),
            region: Region(region),
            scheme: scheme::SAML.to_string(),
            subject_key: name_id,
        })
    }
}

/// Collect the SAML `<AttributeStatement>/<Attribute Name=…>` → first `<AttributeValue>` text map of
/// an assertion (used for the tenant/region trust-rooted facts). Only direct AttributeStatements of
/// the assertion are read (schema position).
fn saml_attributes(assertion: &Element, inherited: &NsScope) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let a_scope = {
        let mut s = inherited.clone();
        for (p, u) in &assertion.ns_decls {
            s.insert(p.clone(), u.clone());
        }
        s
    };
    for stmt in children_named(assertion, inherited, SAML_NS, "AttributeStatement") {
        let stmt_scope = {
            let mut s = a_scope.clone();
            for (p, u) in &stmt.ns_decls {
                s.insert(p.clone(), u.clone());
            }
            s
        };
        for attr in children_named(stmt, &a_scope, SAML_NS, "Attribute") {
            let name = attr
                .attrs
                .iter()
                .find(|a| a.prefix.is_empty() && a.local == "Name")
                .map(|a| a.value.clone());
            if let Some(name) = name {
                if let Some(val) = children_named(attr, &stmt_scope, SAML_NS, "AttributeValue")
                    .first()
                    .map(|v| v.text().trim().to_string())
                {
                    out.entry(name).or_insert(val);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    // The MR-010b SAML XML-DSig corpus: REAL signed assertions (a test IdP keypair mints genuine
    // RSA-SHA256 / ECDSA-SHA256 signatures over real exclusive-c14n bytes), plus the full XSW + forgery
    // negative corpus — each asserted REFUSED. The signer reuses the verifier's OWN `parse_xml` +
    // `canonicalize`, so the positive cases are a real crypto round-trip and the negatives are REAL
    // attacks (tampering breaks a real digest/signature; XSW shapes are real wrapped documents).

    use super::*;
    use crate::authenticate::{scheme, CredentialVerifier, StructuralVerifier};
    use crate::oidc::{JwkKey, ReplayGuard, SchemeDispatchVerifier};
    use base64::engine::general_purpose::STANDARD as TB64;
    use myelin_identity::{AuthzError, Credential};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::Arc;

    const NOW: i64 = 1_700_000_000;
    // A validity window comfortably around NOW (RFC3339 instants — parsed, not lexical-compared).
    const NB: &str = "2023-11-14T20:00:00Z"; // ≈ NOW - ~1300s
    const NA: &str = "2023-11-15T20:00:00Z"; // ≈ NOW + a day
    const ISSUER: &str = "https://idp.example.com/saml";
    const SP: &str = "https://myelin.example.com/sp";

    // ── Test IdP keypairs (the verifier only ever sees the PUBLIC half via the injected trust anchor) ──

    struct RsaSigner {
        priv_key: rsa::RsaPrivateKey,
    }
    impl RsaSigner {
        fn generate() -> RsaSigner {
            use rand::rngs::OsRng;
            RsaSigner {
                priv_key: rsa::RsaPrivateKey::new(&mut OsRng, 2048).expect("rsa keygen"),
            }
        }
        fn jwk(&self) -> JwkKey {
            use rsa::traits::PublicKeyParts;
            let p = self.priv_key.to_public_key();
            JwkKey::Rsa {
                n: p.n().to_bytes_be(),
                e: p.e().to_bytes_be(),
            }
        }
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            use rsa::pkcs1v15::SigningKey;
            use rsa::signature::{SignatureEncoding, Signer};
            use sha2::Sha256;
            SigningKey::<Sha256>::new(self.priv_key.clone())
                .sign(msg)
                .to_vec()
        }
    }

    struct EcSigner {
        pair: ring::signature::EcdsaKeyPair,
        rng: ring::rand::SystemRandom,
        public: Vec<u8>,
    }
    impl EcSigner {
        fn generate() -> EcSigner {
            use ring::rand::SystemRandom;
            use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).expect("ec keygen");
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .expect("ec from pkcs8");
            let public = pair.public_key().as_ref().to_vec();
            EcSigner { pair, rng, public }
        }
        fn jwk(&self) -> JwkKey {
            assert_eq!(self.public.len(), 65, "uncompressed P-256 point");
            JwkKey::EcP256 {
                x: self.public[1..33].to_vec(),
                y: self.public[33..65].to_vec(),
            }
        }
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            self.pair.sign(&self.rng, msg).expect("ec sign").as_ref().to_vec()
        }
    }

    // ── The signed-document builder (mints a genuine signature via the verifier's own c14n) ───────────

    #[allow(clippy::too_many_arguments)]
    fn build_doc(
        assertion_id: &str,
        issuer: &str,
        nameid: &str,
        tenant: &str,
        region: &str,
        not_before: &str,
        not_on_or_after: &str,
        audience: &str,
        sig_method_uri: &str,
        sign: &dyn Fn(&[u8]) -> Vec<u8>,
    ) -> String {
        let signed_info = format!(
            "<ds:SignedInfo xmlns:ds=\"{ds}\">\
             <ds:CanonicalizationMethod Algorithm=\"{exc}\"></ds:CanonicalizationMethod>\
             <ds:SignatureMethod Algorithm=\"{method}\"></ds:SignatureMethod>\
             <ds:Reference URI=\"#{id}\">\
             <ds:Transforms>\
             <ds:Transform Algorithm=\"{env}\"></ds:Transform>\
             <ds:Transform Algorithm=\"{exc}\"></ds:Transform>\
             </ds:Transforms>\
             <ds:DigestMethod Algorithm=\"{sha}\"></ds:DigestMethod>\
             <ds:DigestValue>@@DIGEST@@</ds:DigestValue>\
             </ds:Reference>\
             </ds:SignedInfo>",
            ds = DS_NS,
            exc = EXC_C14N,
            method = sig_method_uri,
            id = assertion_id,
            env = ENVELOPED,
            sha = SHA256_DIGEST,
        );
        // The KeyInfo carries an ATTACKER-CONTROLLED cert string — the verifier must IGNORE it (the
        // trust root is the injected anchor, never the document's KeyInfo).
        let signature = format!(
            "<ds:Signature xmlns:ds=\"{ds}\">{si}<ds:SignatureValue>@@SIG@@</ds:SignatureValue>\
             <ds:KeyInfo><ds:X509Data><ds:X509Certificate>ATTACKER-CONTROLLED-IGNORED</ds:X509Certificate>\
             </ds:X509Data></ds:KeyInfo></ds:Signature>",
            ds = DS_NS,
            si = signed_info,
        );
        let assertion = format!(
            "<saml:Assertion xmlns:saml=\"{s}\" ID=\"{id}\" Version=\"2.0\" IssueInstant=\"{nb}\">\
             <saml:Issuer>{issuer}</saml:Issuer>\
             {sig}\
             <saml:Subject><saml:NameID>{nameid}</saml:NameID></saml:Subject>\
             <saml:Conditions NotBefore=\"{nb}\" NotOnOrAfter=\"{na}\">\
             <saml:AudienceRestriction><saml:Audience>{aud}</saml:Audience></saml:AudienceRestriction>\
             </saml:Conditions>\
             <saml:AttributeStatement>\
             <saml:Attribute Name=\"tenant\"><saml:AttributeValue>{tenant}</saml:AttributeValue></saml:Attribute>\
             <saml:Attribute Name=\"region\"><saml:AttributeValue>{region}</saml:AttributeValue></saml:Attribute>\
             </saml:AttributeStatement>\
             </saml:Assertion>",
            s = SAML_NS,
            id = assertion_id,
            nb = not_before,
            issuer = issuer,
            sig = signature,
            nameid = nameid,
            na = not_on_or_after,
            aud = audience,
            tenant = tenant,
            region = region,
        );
        let doc = format!(
            "<samlp:Response xmlns:samlp=\"urn:oasis:names:tc:SAML:2.0:protocol\" ID=\"_resp1\" \
             Version=\"2.0\" IssueInstant=\"{nb}\">{assertion}</samlp:Response>",
            nb = not_before,
            assertion = assertion,
        );
        finalize(doc, sign)
    }

    /// Insert the real digest (over the enveloped-c14n assertion) and the real signature (over the c14n
    /// SignedInfo), computed with the verifier's OWN code — so the signed bytes EXACTLY match what the
    /// verifier canonicalizes.
    fn finalize(doc: String, sign: &dyn Fn(&[u8]) -> Vec<u8>) -> String {
        use sha2::{Digest, Sha256};

        // (1) Digest over the assertion with the Signature removed (the enveloped transform).
        let parsed = parse_xml(&doc).expect("builder doc parses");
        let mut a = Vec::new();
        collect_named(&parsed, &base_scope(), SAML_NS, "Assertion", &mut a);
        let (assertion, inherited) = a.remove(0);
        let mut c = String::new();
        canonicalize(assertion, &inherited, &NsScope::new(), true, &mut c);
        let digest = Sha256::digest(c.as_bytes()).to_vec();
        let doc = doc.replace("@@DIGEST@@", &TB64.encode(&digest));

        // (2) Signature over the canonicalized SignedInfo (now carrying the real digest).
        let parsed = parse_xml(&doc).expect("re-parse with digest");
        let mut si = Vec::new();
        collect_named(&parsed, &base_scope(), DS_NS, "SignedInfo", &mut si);
        let (signed_info, si_inherited) = si.remove(0);
        let mut sc = String::new();
        canonicalize(signed_info, &si_inherited, &NsScope::new(), false, &mut sc);
        let sig = sign(sc.as_bytes());
        doc.replace("@@SIG@@", &TB64.encode(&sig))
    }

    fn cred(material: String) -> Credential {
        Credential {
            scheme: scheme::SAML.into(),
            material,
        }
    }

    fn config(anchors: Vec<JwkKey>) -> SamlConfig {
        SamlConfig::new(ISSUER, SP, anchors)
    }

    fn verifier(anchors: Vec<JwkKey>) -> SamlVerifier {
        SamlVerifier::new(config(anchors)).with_clock(|| NOW)
    }

    /// A standard, correctly-signed RSA-SHA256 document for tenant `acme` / region `eu-west` / NameID
    /// `alice@acme.example`, paired with the verifier whose trust anchor is the signing key.
    fn rsa_signed(id: &str) -> (SamlVerifier, String) {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            id,
            ISSUER,
            "alice@acme.example",
            "acme",
            "eu-west",
            NB,
            NA,
            SP,
            RSA_SHA256,
            &|m| signer.sign(m),
        );
        (v, doc)
    }

    // ════════════════════════════════════════════════════════════════════════════════════════════════
    // POSITIVE corpus — a correctly-signed RSA-SHA256 (and ECDSA-SHA256) assertion VERIFIES and yields
    // the right tenant/region/subject(NameID) from the SIGNED assertion.
    // ════════════════════════════════════════════════════════════════════════════════════════════════

    #[test]
    fn positive_rsa_sha256_verifies_and_yields_trust_rooted_assertion() {
        let (v, doc) = rsa_signed("_assertion_1");
        let a = v.verify(&cred(doc)).expect("a correctly-signed RSA-SHA256 assertion must verify");
        assert_eq!(a.tenant, TenantId("acme".into()));
        assert_eq!(a.region, Region("eu-west".into()));
        assert_eq!(a.scheme, scheme::SAML);
        assert_eq!(a.subject_key, "alice@acme.example", "subject = the NameID");
    }

    #[test]
    fn positive_ecdsa_sha256_verifies() {
        let signer = EcSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            "_ec_1", ISSUER, "bob@acme.example", "acme", "eu-west", NB, NA, SP, ECDSA_SHA256,
            &|m| signer.sign(m),
        );
        let a = v.verify(&cred(doc)).expect("a correctly-signed ECDSA-SHA256 assertion must verify");
        assert_eq!(a.tenant, TenantId("acme".into()));
        assert_eq!(a.subject_key, "bob@acme.example");
    }

    #[test]
    fn positive_region_falls_back_to_configured_idp_binding() {
        // An assertion carrying NO region attribute resolves region from the configured IdP binding.
        let signer = RsaSigner::generate();
        let v = SamlVerifier::new(config(vec![signer.jwk()]).with_region_default("ap-south"))
            .with_clock(|| NOW);
        let raw = format!(
            "<samlp:Response xmlns:samlp=\"urn:oasis:names:tc:SAML:2.0:protocol\" ID=\"_resp2\" \
             Version=\"2.0\" IssueInstant=\"{NB}\">\
             <saml:Assertion xmlns:saml=\"{SAML_NS}\" ID=\"_rgn_2\" Version=\"2.0\" IssueInstant=\"{NB}\">\
             <saml:Issuer>{ISSUER}</saml:Issuer>\
             <ds:Signature xmlns:ds=\"{DS_NS}\">\
             <ds:SignedInfo xmlns:ds=\"{DS_NS}\">\
             <ds:CanonicalizationMethod Algorithm=\"{EXC_C14N}\"></ds:CanonicalizationMethod>\
             <ds:SignatureMethod Algorithm=\"{RSA_SHA256}\"></ds:SignatureMethod>\
             <ds:Reference URI=\"#_rgn_2\"><ds:Transforms>\
             <ds:Transform Algorithm=\"{ENVELOPED}\"></ds:Transform>\
             <ds:Transform Algorithm=\"{EXC_C14N}\"></ds:Transform></ds:Transforms>\
             <ds:DigestMethod Algorithm=\"{SHA256_DIGEST}\"></ds:DigestMethod>\
             <ds:DigestValue>@@DIGEST@@</ds:DigestValue></ds:Reference></ds:SignedInfo>\
             <ds:SignatureValue>@@SIG@@</ds:SignatureValue></ds:Signature>\
             <saml:Subject><saml:NameID>carol@acme.example</saml:NameID></saml:Subject>\
             <saml:Conditions NotBefore=\"{NB}\" NotOnOrAfter=\"{NA}\">\
             <saml:AudienceRestriction><saml:Audience>{SP}</saml:Audience></saml:AudienceRestriction>\
             </saml:Conditions>\
             <saml:AttributeStatement>\
             <saml:Attribute Name=\"tenant\"><saml:AttributeValue>acme</saml:AttributeValue></saml:Attribute>\
             </saml:AttributeStatement></saml:Assertion></samlp:Response>",
        );
        let doc = finalize(raw, &|m| signer.sign(m));
        let a = v.verify(&cred(doc)).expect("region-less assertion verifies with the fallback region");
        assert_eq!(a.region, Region("ap-south".into()), "region from the configured IdP binding");
        assert_eq!(a.tenant, TenantId("acme".into()));
    }

    // ════════════════════════════════════════════════════════════════════════════════════════════════
    // XSW corpus — the #1 SAML auth-bypass class. Each forgery uses a REAL signed assertion + a real XSW
    // shape, asserted REFUSED.
    // ════════════════════════════════════════════════════════════════════════════════════════════════

    /// A forged assertion (attacker tenant `globex`) — unsigned — to splice into XSW shapes.
    fn forged_assertion(id: &str) -> String {
        format!(
            "<saml:Assertion xmlns:saml=\"{SAML_NS}\" ID=\"{id}\" Version=\"2.0\" IssueInstant=\"{NB}\">\
             <saml:Issuer>{ISSUER}</saml:Issuer>\
             <saml:Subject><saml:NameID>attacker@globex.evil</saml:NameID></saml:Subject>\
             <saml:Conditions NotBefore=\"{NB}\" NotOnOrAfter=\"{NA}\">\
             <saml:AudienceRestriction><saml:Audience>{SP}</saml:Audience></saml:AudienceRestriction>\
             </saml:Conditions>\
             <saml:AttributeStatement>\
             <saml:Attribute Name=\"tenant\"><saml:AttributeValue>globex</saml:AttributeValue></saml:Attribute>\
             <saml:Attribute Name=\"region\"><saml:AttributeValue>eu-west</saml:AttributeValue></saml:Attribute>\
             </saml:AttributeStatement></saml:Assertion>"
        )
    }

    /// XSW-1: a forged assertion as a SIBLING of the original signed assertion. Two assertions → refused.
    #[test]
    fn xsw_1_forged_sibling_assertion_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_1");
        let attack = doc.replace(
            "</samlp:Response>",
            &format!("{}</samlp:Response>", forged_assertion("_forged")),
        );
        let err = v.verify(&cred(attack)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("XSW") && m.contains("Assertion")),
            "a forged sibling assertion must be refused, got {err:?}"
        );
    }

    /// XSW-2: the forged assertion WRAPS the signed one. Two assertions present → refused.
    #[test]
    fn xsw_2_forged_assertion_wrapping_the_signed_one_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_1");
        // Pull the signed assertion out and wrap it inside a forged assertion.
        let start = doc.find("<saml:Assertion ").unwrap();
        let end = doc.find("</saml:Assertion>").unwrap() + "</saml:Assertion>".len();
        let signed = &doc[start..end];
        let wrapped = format!(
            "<saml:Assertion xmlns:saml=\"{SAML_NS}\" ID=\"_wrapper\" Version=\"2.0\" IssueInstant=\"{NB}\">\
             <saml:Issuer>{ISSUER}</saml:Issuer>\
             <saml:Subject><saml:NameID>attacker@globex.evil</saml:NameID></saml:Subject>\
             <saml:Conditions NotBefore=\"{NB}\" NotOnOrAfter=\"{NA}\">\
             <saml:AudienceRestriction><saml:Audience>{SP}</saml:Audience></saml:AudienceRestriction>\
             </saml:Conditions>\
             <saml:AttributeStatement>\
             <saml:Attribute Name=\"tenant\"><saml:AttributeValue>globex</saml:AttributeValue></saml:Attribute>\
             </saml:AttributeStatement>{signed}</saml:Assertion>"
        );
        let attack = format!("{}{}{}", &doc[..start], wrapped, &doc[end..]);
        let err = v.verify(&cred(attack)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("XSW")),
            "a forged wrapping assertion must be refused, got {err:?}"
        );
    }

    /// XSW-3: the signed assertion is moved into a `<samlp:Extensions>` wrapper while a forged assertion
    /// takes the asserted position. Two assertions → refused.
    #[test]
    fn xsw_3_signed_moved_into_extensions_forged_in_position_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_1");
        let start = doc.find("<saml:Assertion ").unwrap();
        let end = doc.find("</saml:Assertion>").unwrap() + "</saml:Assertion>".len();
        let signed = &doc[start..end];
        let attack = format!(
            "{}<samlp:Extensions>{}</samlp:Extensions>{}{}",
            &doc[..start],
            signed,
            forged_assertion("_forged"),
            &doc[end..]
        );
        let err = v.verify(&cred(attack)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("XSW")),
            "moving the signed assertion into Extensions must be refused, got {err:?}"
        );
    }

    /// XSW-4: DUPLICATE ID — a non-assertion element carries the SAME ID as the signed assertion, so the
    /// Reference `#id` resolves to two elements. Refused by the duplicate-ID guard (before any digest).
    #[test]
    fn xsw_4_duplicate_id_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_1");
        // A second element (NOT an Assertion, so the single-assertion guard does not fire first) with the
        // SAME ID as the signed assertion.
        let attack = doc.replace(
            "</samlp:Response>",
            "<samlp:Extensions ID=\"_assertion_1\"></samlp:Extensions></samlp:Response>",
        );
        let err = v.verify(&cred(attack)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("ID") && m.contains("unique")),
            "a duplicate signed-element ID must be refused, got {err:?}"
        );
    }

    /// XSW-5: the Reference is RE-POINTED at a different (non-assertion) element that the attacker adds —
    /// so the signature references element A while claims would be read from the assertion. Refused
    /// because the signed Reference does not point at the assertion the claims come from.
    #[test]
    fn xsw_5_reference_reassigned_away_from_assertion_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_1");
        let attack = doc
            .replace(
                "</samlp:Response>",
                "<samlp:Extensions ID=\"_decoy\"></samlp:Extensions></samlp:Response>",
            )
            .replace("URI=\"#_assertion_1\"", "URI=\"#_decoy\"");
        let err = v.verify(&cred(attack)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("XSW") || m.contains("does not point")),
            "a Reference re-pointed away from the assertion must be refused, got {err:?}"
        );
    }

    /// XSW (position): the `<ds:Signature>` is moved OUT of the assertion to be a child of the Response
    /// (detached). Refused by the enveloped schema-position guard.
    #[test]
    fn xsw_signature_not_enveloped_child_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_1");
        let start = doc.find("<ds:Signature ").unwrap();
        let end = doc.find("</ds:Signature>").unwrap() + "</ds:Signature>".len();
        let sig = doc[start..end].to_string();
        // Remove the signature from the assertion, re-attach it as a child of the Response.
        let without = format!("{}{}", &doc[..start], &doc[end..]);
        let attack = without.replace("</saml:Assertion>", &format!("</saml:Assertion>{sig}"));
        let err = v.verify(&cred(attack)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("schema-position") || m.contains("enveloped")),
            "a non-enveloped signature must be refused, got {err:?}"
        );
    }

    // ════════════════════════════════════════════════════════════════════════════════════════════════
    // Other forgery / failure corpus.
    // ════════════════════════════════════════════════════════════════════════════════════════════════

    /// TAMPERED — the tenant claim is edited AFTER signing (`acme`→`globex`). The canonicalized assertion
    /// no longer matches the DigestValue → refused. The load-bearing IDOR/forgery case.
    #[test]
    fn tampered_tenant_after_signing_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_1");
        let attack = doc.replace(
            "<saml:AttributeValue>acme</saml:AttributeValue>",
            "<saml:AttributeValue>globex</saml:AttributeValue>",
        );
        let err = v.verify(&cred(attack)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("digest mismatch")),
            "a tampered tenant must be refused (digest mismatch), got {err:?}"
        );
    }

    /// NON-ANCHORED CERT — the assertion is signed by an ATTACKER key (whose cert the attacker also put
    /// in KeyInfo), but the configured trust anchor is the VICTIM IdP key. The signature must fail
    /// against the anchor (the document's KeyInfo is never trusted).
    #[test]
    fn signature_by_non_anchored_key_is_rejected() {
        let victim = RsaSigner::generate();
        let attacker = RsaSigner::generate();
        // Anchor = the victim IdP key; document signed by the attacker.
        let v = verifier(vec![victim.jwk()]);
        let doc = build_doc(
            "_assertion_1", ISSUER, "attacker@globex.evil", "globex", "eu-west", NB, NA, SP, RSA_SHA256,
            &|m| attacker.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("verification failed")),
            "a signature by a non-anchored key must be refused, got {err:?}"
        );
    }

    /// SHA-1 — a `rsa-sha1` SignatureMethod is refused at parse (the SHA-1 downgrade defence), regardless
    /// of the signature bytes.
    #[test]
    fn sha1_signature_method_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            "_assertion_1", ISSUER, "alice@acme.example", "acme", "eu-west", NB, NA, SP,
            "http://www.w3.org/2000/09/xmldsig#rsa-sha1",
            &|m| signer.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("SHA-1") || m.contains("rsa-sha1")),
            "a SHA-1 signature method must be refused, got {err:?}"
        );
    }

    /// UNSIGNED — an assertion with NO signature at all. Refused (zero signatures).
    #[test]
    fn unsigned_assertion_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = format!(
            "<samlp:Response xmlns:samlp=\"urn:oasis:names:tc:SAML:2.0:protocol\" ID=\"_r\" \
             Version=\"2.0\" IssueInstant=\"{NB}\">{}</samlp:Response>",
            forged_assertion("_unsigned")
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("Signature")),
            "an unsigned assertion must be refused, got {err:?}"
        );
    }

    /// EXPIRED — NotOnOrAfter in the past beyond leeway. Parsed to an instant, refused.
    #[test]
    fn expired_assertion_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            "_assertion_1", ISSUER, "alice@acme.example", "acme", "eu-west",
            "2000-01-01T00:00:00Z", "2000-01-02T00:00:00Z", SP, RSA_SHA256,
            &|m| signer.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("expired")),
            "an expired assertion must be refused, got {err:?}"
        );
    }

    /// NOT-YET-VALID — NotBefore in the future beyond leeway. Refused.
    #[test]
    fn not_yet_valid_assertion_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            "_assertion_1", ISSUER, "alice@acme.example", "acme", "eu-west",
            "2099-01-01T00:00:00Z", "2099-01-02T00:00:00Z", SP, RSA_SHA256,
            &|m| signer.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("not yet valid")),
            "a not-yet-valid assertion must be refused, got {err:?}"
        );
    }

    /// WRONG AUDIENCE — the assertion is for some other SP. Refused.
    #[test]
    fn wrong_audience_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            "_assertion_1", ISSUER, "alice@acme.example", "acme", "eu-west", NB, NA,
            "https://some-other-sp.example.com", RSA_SHA256,
            &|m| signer.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("audience")),
            "a wrong-audience assertion must be refused, got {err:?}"
        );
    }

    /// WRONG ISSUER — the assertion Issuer is not the configured IdP. Refused.
    #[test]
    fn wrong_issuer_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            "_assertion_1", "https://evil-idp.example.com", "alice@acme.example", "acme", "eu-west",
            NB, NA, SP, RSA_SHA256,
            &|m| signer.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("issuer")),
            "a wrong-issuer assertion must be refused, got {err:?}"
        );
    }

    /// REPLAY — the SAME assertion presented twice. First verifies; the second (same ID) refused.
    #[test]
    fn replayed_assertion_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_replay");
        v.verify(&cred(doc.clone())).expect("first presentation verifies");
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("replay")),
            "a replayed assertion must be refused, got {err:?}"
        );
    }

    /// COMMENT-INJECTION in NameID — the `;;`/c14n-comment trick: a comment node splits the NameID text.
    /// Comments are rejected at parse, so the document is refused outright.
    #[test]
    fn nameid_comment_injection_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_1");
        let attack = doc.replace(
            "<saml:NameID>alice@acme.example</saml:NameID>",
            "<saml:NameID>alice@acme.example<!---->.admin@evil</saml:NameID>",
        );
        let err = v.verify(&cred(attack)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::BadRequest(m) if m.contains("comment")),
            "a NameID comment injection must be refused, got {err:?}"
        );
    }

    /// XXE — an external-entity DOCTYPE. Refused at parse (no entity processing).
    #[test]
    fn xxe_external_entity_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let attack = format!(
            "<?xml version=\"1.0\"?><!DOCTYPE foo [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]>\
             <samlp:Response xmlns:samlp=\"urn:oasis:names:tc:SAML:2.0:protocol\">\
             <saml:Assertion xmlns:saml=\"{SAML_NS}\" ID=\"_a\"><saml:Issuer>&xxe;</saml:Issuer>\
             </saml:Assertion></samlp:Response>"
        );
        let err = v.verify(&cred(attack)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::BadRequest(m) if m.contains("DTD") || m.contains("entity")),
            "an XXE DOCTYPE must be refused, got {err:?}"
        );
    }

    /// BILLION LAUGHS — nested entity-expansion DOCTYPE. Refused at parse (no entity expansion).
    #[test]
    fn billion_laughs_entity_expansion_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let attack = "<?xml version=\"1.0\"?><!DOCTYPE lolz [\
             <!ENTITY lol \"lol\">\
             <!ENTITY lol2 \"&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;\">\
             <!ENTITY lol3 \"&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;\">\
             ]><lolz>&lol3;</lolz>"
            .to_string();
        let err = v.verify(&cred(attack)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::BadRequest(m) if m.contains("DTD") || m.contains("entity")),
            "a billion-laughs DOCTYPE must be refused, got {err:?}"
        );
    }

    /// MALFORMED / GARBAGE + FUZZ — `verify` is TOTAL over attacker bytes: every garbage input is a loud
    /// refusal, NEVER a panic.
    #[test]
    fn malformed_and_fuzz_inputs_are_refused_not_panicking() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let cases: Vec<String> = vec![
            String::new(),
            "not xml at all".into(),
            "<".into(),
            "<unclosed".into(),
            "<a><b></a>".into(),
            "<a attr=>".into(),
            "<a>&undefinedentity;</a>".into(),
            "<a>&#xZZZ;</a>".into(),
            "<a>&".into(),
            "<saml:Assertion".into(),
            "<root>".to_string() + &"<x>".repeat(5000), // deep nesting (no stack-busting panic / total)
            "\u{0}\u{1}\u{2}garbage".into(),
            "<a xmlns:ds=\"\">".into(),
            "<ds:Signature></ds:Signature>".into(),
            "<samlp:Response xmlns:samlp=\"urn:oasis:names:tc:SAML:2.0:protocol\"></samlp:Response>".into(),
        ];
        for (i, bad) in cases.iter().enumerate() {
            let r = v.verify(&cred(bad.clone()));
            assert!(r.is_err(), "garbage case {i} must be refused (and must not panic): {bad:?}");
        }
    }

    /// DEEP NESTING (DoS) — a WELL-FORMED, matched-close-tag document nested far deeper than the
    /// bound must be REFUSED at parse with the depth error, NEVER crash. This is the case the in-tree
    /// fuzz corpus missed (it used UNCLOSED nesting, which stays in the parser's flat Vec); a balanced
    /// document is what overflows the recursive DOM traversal / canonicalize / Drop. 5000 deep is the
    /// verifier's prior SIGABRT crash point. `verify` must stay TOTAL over attacker bytes.
    #[test]
    fn deeply_nested_well_formed_document_is_refused_not_crashing() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let depth = 5000;
        let mut doc = String::with_capacity(depth * 8);
        for _ in 0..depth {
            doc.push_str("<n>");
        }
        for _ in 0..depth {
            doc.push_str("</n>");
        }
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::BadRequest(m) if m.contains("nesting too deep")),
            "a deeply-nested well-formed document must be refused with the depth error, got {err:?}"
        );
        // And a normal shallow assertion still verifies (the bound never bites a real assertion).
        let (v2, ok) = rsa_signed("_assertion_depth_ok");
        assert!(v2.verify(&cred(ok)).is_ok(), "a shallow assertion must still verify");
    }

    /// EMPTY / MISSING tenant — a verified assertion that carries no tenant attribute is refused (we never
    /// fabricate a tenant — the tenant is the trust root, ID-3).
    #[test]
    fn missing_tenant_attribute_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let raw = format!(
            "<samlp:Response xmlns:samlp=\"urn:oasis:names:tc:SAML:2.0:protocol\" ID=\"_r\" \
             Version=\"2.0\" IssueInstant=\"{NB}\">\
             <saml:Assertion xmlns:saml=\"{SAML_NS}\" ID=\"_a\" Version=\"2.0\" IssueInstant=\"{NB}\">\
             <saml:Issuer>{ISSUER}</saml:Issuer>\
             <ds:Signature xmlns:ds=\"{DS_NS}\"><ds:SignedInfo xmlns:ds=\"{DS_NS}\">\
             <ds:CanonicalizationMethod Algorithm=\"{EXC_C14N}\"></ds:CanonicalizationMethod>\
             <ds:SignatureMethod Algorithm=\"{RSA_SHA256}\"></ds:SignatureMethod>\
             <ds:Reference URI=\"#_a\"><ds:Transforms>\
             <ds:Transform Algorithm=\"{ENVELOPED}\"></ds:Transform>\
             <ds:Transform Algorithm=\"{EXC_C14N}\"></ds:Transform></ds:Transforms>\
             <ds:DigestMethod Algorithm=\"{SHA256_DIGEST}\"></ds:DigestMethod>\
             <ds:DigestValue>@@DIGEST@@</ds:DigestValue></ds:Reference></ds:SignedInfo>\
             <ds:SignatureValue>@@SIG@@</ds:SignatureValue></ds:Signature>\
             <saml:Subject><saml:NameID>alice@acme.example</saml:NameID></saml:Subject>\
             <saml:Conditions NotBefore=\"{NB}\" NotOnOrAfter=\"{NA}\">\
             <saml:AudienceRestriction><saml:Audience>{SP}</saml:Audience></saml:AudienceRestriction>\
             </saml:Conditions></saml:Assertion></samlp:Response>"
        );
        let doc = finalize(raw, &|m| signer.sign(m));
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("tenant")),
            "an assertion with no tenant attribute must be refused, got {err:?}"
        );
    }

    // ── The dispatch seam ────────────────────────────────────────────────────────────────────────────

    /// The dispatcher routes the SAML scheme to the REAL [`SamlVerifier`]; a forged (unsigned) SAML
    /// credential hits the real verifier and is refused, NOT silently accepted by the floor fallback.
    /// (Construction of the floor `StructuralVerifier` here is `#[cfg(test)]`, so the production-graph
    /// scanner admits it.)
    #[test]
    fn dispatch_routes_saml_to_real_verifier() {
        let (v, doc) = rsa_signed("_assertion_dispatch");
        let dispatch = SchemeDispatchVerifier::new(Arc::new(StructuralVerifier::new()))
            .route(scheme::SAML, Arc::new(v));

        // A correctly-signed SAML credential verifies through the real crypto verifier.
        let a = dispatch.verify(&cred(doc)).expect("real SAML verifies via the dispatcher");
        assert_eq!(a.tenant, TenantId("acme".into()));
        assert_eq!(a.scheme, scheme::SAML);

        // A forged (unsigned) SAML credential hits the real verifier and is refused (not the floor).
        let signer = RsaSigner::generate();
        let dispatch2 = SchemeDispatchVerifier::new(Arc::new(StructuralVerifier::new()))
            .route(scheme::SAML, Arc::new(verifier(vec![signer.jwk()])));
        let forged = format!(
            "<samlp:Response xmlns:samlp=\"urn:oasis:names:tc:SAML:2.0:protocol\">{}</samlp:Response>",
            forged_assertion("_forged")
        );
        assert!(
            dispatch2.verify(&cred(forged)).is_err(),
            "an unsigned SAML assertion must hit the real verifier and be refused"
        );
    }

    /// A shared replay guard across verifier handles: an assertion consumed via one handle is a replay on
    /// another sharing the guard.
    #[test]
    fn shared_replay_guard_blocks_cross_handle_replay() {
        let signer = RsaSigner::generate();
        let guard = ReplayGuard::new();
        let v1 = SamlVerifier::new(config(vec![signer.jwk()]))
            .with_clock(|| NOW)
            .with_replay_guard(guard.clone());
        let v2 = SamlVerifier::new(config(vec![signer.jwk()]))
            .with_clock(|| NOW)
            .with_replay_guard(guard);
        let doc = build_doc(
            "_assertion_shared", ISSUER, "alice@acme.example", "acme", "eu-west", NB, NA, SP, RSA_SHA256,
            &|m| signer.sign(m),
        );
        v1.verify(&cred(doc.clone())).expect("first handle verifies");
        let err = v2.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("replay")),
            "the shared guard must block a cross-handle replay, got {err:?}"
        );
    }

}
