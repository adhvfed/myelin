use crate::authenticate::{scheme, CredentialVerifier, VerifiedAssertion};
use crate::oidc::{JwkKey, ReplayGuard};
use myelin_identity::{AuthzError, Credential};
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

const DS_NS: &str = "http://www.w3.org/2000/09/xmldsig#";
const SAML_NS: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
const SAML_PROTOCOL_NS: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
const SAML_BEARER_METHOD: &str = "urn:oasis:names:tc:SAML:2.0:cm:bearer";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

const EXC_C14N: &str = "http://www.w3.org/2001/10/xml-exc-c14n#";
const EXC_C14N_COMMENTS: &str = "http://www.w3.org/2001/10/xml-exc-c14n#WithComments";
const ENVELOPED: &str = "http://www.w3.org/2000/09/xmldsig#enveloped-signature";
const SHA256_DIGEST: &str = "http://www.w3.org/2001/04/xmlenc#sha256";
const RSA_SHA256: &str = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
const ECDSA_SHA256: &str = "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256";

const MAX_NESTING_DEPTH: usize = 256;

fn refuse(msg: impl Into<String>) -> AuthzError {
    AuthzError::FailClosed(msg.into())
}

fn malformed(msg: impl Into<String>) -> AuthzError {
    AuthzError::BadRequest(msg.into())
}

#[derive(Clone, Debug)]
struct Element {
    prefix: String,
    local: String,
    ns_decls: Vec<(String, String)>,
    attrs: Vec<Attr>,
    children: Vec<Node>,
}

#[derive(Clone, Debug)]
struct Attr {
    prefix: String,
    local: String,
    value: String,
}

#[derive(Clone, Debug)]
enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    fn id_attr(&self) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.prefix.is_empty() && a.local == "ID")
            .map(|a| a.value.as_str())
    }

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

fn parse_xml(xml: &str) -> Result<Element, AuthzError> {
    use xmlparser::{ElementEnd, Token};

    let mut stack: Vec<Element> = Vec::new();
    let mut pending: Option<Element> = None;
    let mut root: Option<Element> = None;

    fn attach(
        el: Element,
        stack: &mut [Element],
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
            Token::Declaration { .. } => {}
            Token::DtdStart { .. } | Token::EmptyDtd { .. } | Token::DtdEnd { .. } => {
                return Err(malformed(
                    "DTD / DOCTYPE is rejected (XXE / entity-expansion defence - no entity processing)",
                ));
            }
            Token::EntityDeclaration { .. } => {
                return Err(malformed(
                    "entity declaration is rejected (XXE / billion-laughs defence)",
                ));
            }
            Token::Comment { .. } => {
                return Err(malformed(
                    "XML comments are rejected (NameID comment-injection / c14n-comment defence)",
                ));
            }
            Token::ProcessingInstruction { .. } => {
                return Err(malformed("processing instructions are rejected"));
            }
            Token::ElementStart { prefix, local, .. } => {
                if pending.is_some() {
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
                    el.ns_decls.push((String::new(), val));
                } else if p == "xmlns" {
                    el.ns_decls.push((l.to_string(), val));
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
                    if stack.len() >= MAX_NESTING_DEPTH {
                        return Err(malformed(format!(
                            "XML nesting too deep (> {MAX_NESTING_DEPTH}) - refused (a deeply-nested \
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
                    parent
                        .children
                        .push(Node::Text(unescape_xml(text.as_str())?));
                }
            }
            Token::Cdata { text, .. } => {
                if let Some(parent) = stack.last_mut() {
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
                     numeric refs are allowed - no entity expansion)"
                )));
            }
        }
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

type NsScope = BTreeMap<String, String>;

fn base_scope() -> NsScope {
    let mut s = NsScope::new();
    s.insert("xml".to_string(), XML_NS.to_string());
    s
}

fn resolve<'a>(scope: &'a NsScope, prefix: &str) -> &'a str {
    scope.get(prefix).map(|s| s.as_str()).unwrap_or("")
}

fn push_qname(out: &mut String, prefix: &str, local: &str) {
    if !prefix.is_empty() {
        out.push_str(prefix);
        out.push(':');
    }
    out.push_str(local);
}

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

fn canonicalize(
    el: &Element,
    inherited: &NsScope,
    rendered: &NsScope,
    skip_signature: bool,
    out: &mut String,
) {
    let mut scope = inherited.clone();
    for (p, u) in &el.ns_decls {
        scope.insert(p.clone(), u.clone());
    }

    let mut utilized: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    utilized.insert(el.prefix.clone());
    for a in &el.attrs {
        if !a.prefix.is_empty() {
            utilized.insert(a.prefix.clone());
        }
    }

    let mut to_render: Vec<(String, String)> = Vec::new();
    for p in &utilized {
        if p == "xml" {
            continue;
        }
        let uri = resolve(&scope, p).to_string();
        if p.is_empty() && uri.is_empty() {
            if rendered.get("").is_some_and(|u| !u.is_empty()) {
                to_render.push((String::new(), String::new()));
            }
            continue;
        }
        match rendered.get(p) {
            Some(r) if r == &uri => {}
            _ => to_render.push((p.clone(), uri)),
        }
    }
    to_render.sort_by(|a, b| a.0.cmp(&b.0));

    let mut child_rendered = rendered.clone();
    for (p, u) in &to_render {
        child_rendered.insert(p.clone(), u.clone());
    }

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

    for child in &el.children {
        match child {
            Node::Text(t) => push_text(out, t),
            Node::Element(c) => {
                if skip_signature && c.local == "Signature" && child_ns_matches(c, &scope) == DS_NS
                {
                    continue;
                }
                canonicalize(c, &scope, &child_rendered, skip_signature, out);
            }
        }
    }

    out.push_str("</");
    push_qname(out, &el.prefix, &el.local);
    out.push('>');
}

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

fn child_named<'a>(
    parent: &'a Element,
    scope: &NsScope,
    ns: &str,
    local: &str,
) -> Option<&'a Element> {
    let mut s = scope.clone();
    for (p, u) in &parent.ns_decls {
        s.insert(p.clone(), u.clone());
    }
    parent.children.iter().find_map(|c| match c {
        Node::Element(e) if child_ns_matches(e, &s) == ns && e.local == local => Some(e),
        _ => None,
    })
}

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

fn algorithm(el: &Element) -> Option<&str> {
    el.attrs
        .iter()
        .find(|a| a.prefix.is_empty() && a.local == "Algorithm")
        .map(|a| a.value.as_str())
}

type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct SamlConfig {
    pub issuer: String,
    pub sp_entity_id: String,
    pub trust_anchors: Vec<JwkKey>,
    pub tenant_attr: String,
    pub region_attr: String,
    pub region_default: Option<String>,
    pub leeway_secs: i64,
}

impl SamlConfig {
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

    pub fn with_attrs(
        mut self,
        tenant_attr: impl Into<String>,
        region_attr: impl Into<String>,
    ) -> SamlConfig {
        self.tenant_attr = tenant_attr.into();
        self.region_attr = region_attr.into();
        self
    }

    pub fn with_region_default(mut self, region: impl Into<String>) -> SamlConfig {
        self.region_default = Some(region.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SamlRequestBinding {
    pub acs_url: String,
    pub request_id: String,
}

impl SamlRequestBinding {
    pub fn new(
        acs_url: impl Into<String>,
        request_id: impl Into<String>,
    ) -> SamlRequestBinding {
        SamlRequestBinding {
            acs_url: acs_url.into(),
            request_id: request_id.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SigMethod {
    RsaSha256,
    EcdsaSha256,
}

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
                return Err(refuse(
                    "invalid P-256 trust-anchor coordinates (expected 32 bytes each)",
                ));
            }
            let mut point = Vec::with_capacity(65);
            point.push(0x04);
            point.extend_from_slice(x);
            point.extend_from_slice(y);
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
                .verify(signed_info_c14n, sig)
                .map_err(|_| refuse("XML-DSig ecdsa-sha256 signature verification failed"))
        }
        _ => Err(refuse(
            "signature method does not match the configured trust-anchor key family \
             (alg-confusion defence)",
        )),
    }
}

fn b64_decode_ws(s: &str) -> Result<Vec<u8>, AuthzError> {
    let compact: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    B64.decode(compact.as_bytes())
        .map_err(|e| malformed(format!("malformed base64 in signature/digest: {e}")))
}

#[derive(Clone)]
pub struct SamlVerifier {
    config: SamlConfig,
    replay: ReplayGuard,
    request_binding: Option<SamlRequestBinding>,
    now: NowFn,
}

impl SamlVerifier {
    pub fn new(config: SamlConfig, replay: ReplayGuard) -> SamlVerifier {
        SamlVerifier {
            config,
            replay,
            request_binding: None,
            now: Arc::new(system_now),
        }
    }

    pub fn with_request_binding(mut self, binding: SamlRequestBinding) -> SamlVerifier {
        self.request_binding = Some(binding);
        self
    }

    pub fn with_clock(mut self, now: impl Fn() -> i64 + Send + Sync + 'static) -> SamlVerifier {
        self.now = Arc::new(now);
        self
    }

    pub fn replay_guard(&self) -> &ReplayGuard {
        &self.replay
    }

    fn now(&self) -> i64 {
        (self.now)()
    }

    fn instant(value: &str) -> Result<i64, AuthzError> {
        chrono::DateTime::parse_from_rfc3339(value.trim())
            .map(|dt| dt.timestamp())
            .map_err(|e| refuse(format!("unparseable SAML dateTime `{value}`: {e}")))
    }
}

impl CredentialVerifier for SamlVerifier {
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion> {
        if credential.scheme != scheme::SAML {
            return Err(malformed(format!(
                "SamlVerifier received a `{}` credential (expected `saml`)",
                credential.scheme
            )));
        }
        let request_binding = self.request_binding.as_ref().ok_or_else(|| {
            refuse(
                "SAML request binding is absent - verification requires the server-held ACS URL \
                 and AuthnRequest ID",
            )
        })?;
        if request_binding.acs_url.trim().is_empty() || request_binding.request_id.trim().is_empty() {
            return Err(refuse(
                "SAML request binding has an empty ACS URL or AuthnRequest ID",
            ));
        }

        let root = parse_xml(credential.material.trim())?;
        let base = base_scope();
        let mut root_scope = base.clone();
        for (prefix, uri) in &root.ns_decls {
            root_scope.insert(prefix.clone(), uri.clone());
        }
        if resolve(&root_scope, &root.prefix) != SAML_PROTOCOL_NS || root.local != "Response" {
            return Err(refuse(
                "SAML credential root is not a <samlp:Response> protocol element",
            ));
        }

        let mut assertions: Vec<(&Element, NsScope)> = Vec::new();
        collect_named(&root, &base, SAML_NS, "Assertion", &mut assertions);
        if assertions.is_empty() {
            return Err(refuse("no <saml:Assertion> in the document"));
        }
        if assertions.len() > 1 {
            return Err(refuse(format!(
                "XSW defence: {} <saml:Assertion> elements present - exactly one is required (a \
                 wrapped/forged assertion was injected)",
                assertions.len()
            )));
        }
        let (assertion, assertion_inherited) = assertions.remove(0);

        let mut signatures: Vec<(&Element, NsScope)> = Vec::new();
        collect_named(&root, &base, DS_NS, "Signature", &mut signatures);
        if signatures.len() != 1 {
            return Err(refuse(format!(
                "XSW defence: expected exactly one <ds:Signature>, found {}",
                signatures.len()
            )));
        }
        let (signature, sig_inherited) = signatures.remove(0);
        let sig_is_enveloped_child =
            child_named(assertion, &assertion_inherited, DS_NS, "Signature")
                .is_some_and(|c| std::ptr::eq(c as *const Element, signature as *const Element));
        if !sig_is_enveloped_child {
            return Err(refuse(
                "XSW defence: the <ds:Signature> is not the enveloped direct child of the signed \
                 assertion (schema-position violation)",
            ));
        }

        let signed_info = child_named(signature, &sig_inherited, DS_NS, "SignedInfo")
            .ok_or_else(|| refuse("Signature has no SignedInfo"))?;
        let signed_info_scope = {
            let mut s = sig_inherited.clone();
            for (p, u) in &signature.ns_decls {
                s.insert(p.clone(), u.clone());
            }
            s
        };

        let c14n_method = child_named(
            signed_info,
            &signed_info_scope,
            DS_NS,
            "CanonicalizationMethod",
        )
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
            other => return Err(refuse(format!(
                "unsupported/weak SignatureMethod `{other}` (only rsa-sha256 / ecdsa-sha256 are \
                     accepted - SHA-1 is rejected)"
            ))),
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

        let uri = reference
            .attrs
            .iter()
            .find(|a| a.prefix.is_empty() && a.local == "URI")
            .map(|a| a.value.as_str())
            .ok_or_else(|| refuse("Reference has no URI (a detached reference is refused)"))?;
        let ref_id = uri.strip_prefix('#').ok_or_else(|| {
            refuse(format!(
                "Reference URI `{uri}` is not a same-document `#id` fragment (detached/whole-document \
                 references are refused - XSW defence)"
            ))
        })?;
        if ref_id.is_empty() {
            return Err(refuse("Reference URI `#` is empty"));
        }
        let mut by_id: Vec<&Element> = Vec::new();
        collect_by_id(&root, ref_id, &mut by_id);
        if by_id.len() != 1 {
            return Err(refuse(format!(
                "XSW defence: {} elements carry ID `{ref_id}` - the signed-element reference must be \
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
            true,
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
                "no IdP trust anchor configured - cannot verify the SAML signature (fail-closed)",
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

        let now = self.now();
        let leeway = self.config.leeway_secs;
        let assertion_expiry = if let Some(conditions) =
            child_named(assertion, &assertion_inherited, SAML_NS, "Conditions")
        {
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
            let na = conditions
                .attrs
                .iter()
                .find(|a| a.prefix.is_empty() && a.local == "NotOnOrAfter")
                .ok_or_else(|| refuse("signed assertion Conditions has no NotOnOrAfter"))?;
            let na = SamlVerifier::instant(&na.value)?;
            if na.saturating_add(leeway) <= now {
                return Err(refuse(format!(
                    "assertion expired: NotOnOrAfter instant {na} (+{leeway}s) <= now {now}"
                )));
            }
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
            na
        } else {
            return Err(refuse(
                "signed assertion has no <saml:Conditions> (cannot validate validity window / audience)",
            ));
        };

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

        let response_attr = |name: &str| {
            root.attrs
                .iter()
                .find(|attr| attr.prefix.is_empty() && attr.local == name)
                .map(|attr| attr.value.as_str())
        };
        if response_attr("Destination") != Some(request_binding.acs_url.as_str()) {
            return Err(refuse(
                "SAML Response Destination does not match this login's ACS URL",
            ));
        }
        if response_attr("InResponseTo") != Some(request_binding.request_id.as_str()) {
            return Err(refuse(
                "SAML Response InResponseTo does not match the issued AuthnRequest",
            ));
        }

        let confirmations = children_named(subject, &subject_scope, SAML_NS, "SubjectConfirmation");
        let bearer_confirmations: Vec<&Element> = confirmations
            .into_iter()
            .filter(|confirmation| {
                confirmation
                    .attrs
                    .iter()
                    .find(|attr| attr.prefix.is_empty() && attr.local == "Method")
                    .is_some_and(|attr| attr.value == SAML_BEARER_METHOD)
            })
            .collect();
        if bearer_confirmations.len() != 1 {
            return Err(refuse(format!(
                "signed assertion must contain exactly one bearer SubjectConfirmation, found {}",
                bearer_confirmations.len()
            )));
        }
        let confirmation = bearer_confirmations[0];
        let mut confirmation_scope = subject_scope.clone();
        for (prefix, uri) in &subject.ns_decls {
            confirmation_scope.insert(prefix.clone(), uri.clone());
        }
        let confirmation_data = child_named(
            confirmation,
            &confirmation_scope,
            SAML_NS,
            "SubjectConfirmationData",
        )
        .ok_or_else(|| refuse("signed bearer SubjectConfirmation has no SubjectConfirmationData"))?;
        let confirmation_attr = |name: &str| {
            confirmation_data
                .attrs
                .iter()
                .find(|attr| attr.prefix.is_empty() && attr.local == name)
                .map(|attr| attr.value.as_str())
        };
        if confirmation_attr("Recipient") != Some(request_binding.acs_url.as_str()) {
            return Err(refuse(
                "signed SubjectConfirmationData Recipient does not match this login's ACS URL",
            ));
        }
        if confirmation_attr("InResponseTo") != Some(request_binding.request_id.as_str()) {
            return Err(refuse(
                "signed SubjectConfirmationData InResponseTo does not match the issued AuthnRequest",
            ));
        }
        let confirmation_expiry = confirmation_attr("NotOnOrAfter")
            .ok_or_else(|| refuse("signed SubjectConfirmationData has no NotOnOrAfter"))?;
        let confirmation_expiry = SamlVerifier::instant(confirmation_expiry)?;
        if confirmation_expiry.saturating_add(leeway) <= now {
            return Err(refuse(
                "signed SubjectConfirmationData is expired for this ACS delivery",
            ));
        }

        let replay_namespace = serde_json::json!([
            "saml",
            self.config.issuer,
            self.config.sp_entity_id,
            region,
            request_binding.acs_url
        ])
        .to_string();
        if !self.replay.consume_scoped(
            &tenant,
            &replay_namespace,
            ref_id,
            assertion_expiry
                .min(confirmation_expiry)
                .saturating_add(leeway),
            now,
        )? {
            return Err(refuse(
                "replayed SAML assertion: its signed ID was already presented (replay defence)",
            ));
        }

        Ok(VerifiedAssertion {
            tenant: TenantId(tenant),
            region: Region(region),
            scheme: scheme::SAML.to_string(),
            subject_key: name_id,
            expires_at_unix: Some(assertion_expiry.min(confirmation_expiry)),
        })
    }
}

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

    use super::*;
    use crate::authenticate::{scheme, CredentialVerifier, StructuralVerifier};
    use crate::oidc::{JwkKey, ReplayGuard, SchemeDispatchVerifier};
    use base64::engine::general_purpose::STANDARD as TB64;
    use myelin_identity::{AuthzError, Credential};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::Arc;

    const NOW: i64 = 1_700_000_000;
    const NB: &str = "2023-11-14T20:00:00Z";
    const NA: &str = "2023-11-15T20:00:00Z";
    const ISSUER: &str = "https://idp.example.com/saml";
    const SP: &str = "https://myelin.example.com/sp";
    const ACS: &str = "https://myelin.example.com/v1/auth/saml/acs";
    const REQUEST_ID: &str = "_authn_request_1";

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
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                .expect("ec keygen");
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
            self.pair
                .sign(&self.rng, msg)
                .expect("ec sign")
                .as_ref()
                .to_vec()
        }
    }

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
        build_doc_for_binding(
            assertion_id,
            issuer,
            nameid,
            tenant,
            region,
            not_before,
            not_on_or_after,
            audience,
            ACS,
            REQUEST_ID,
            sig_method_uri,
            sign,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_doc_for_binding(
        assertion_id: &str,
        issuer: &str,
        nameid: &str,
        tenant: &str,
        region: &str,
        not_before: &str,
        not_on_or_after: &str,
        audience: &str,
        acs_url: &str,
        request_id_value: &str,
        sig_method_uri: &str,
        sign: &dyn Fn(&[u8]) -> Vec<u8>,
    ) -> String {
        let not_on_or_after_attr = if not_on_or_after.is_empty() {
            String::new()
        } else {
            format!(" NotOnOrAfter=\"{not_on_or_after}\"")
        };
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
             <saml:Subject><saml:NameID>{nameid}</saml:NameID>\
             <saml:SubjectConfirmation Method=\"{bearer}\">\
             <saml:SubjectConfirmationData Recipient=\"{acs}\" InResponseTo=\"{request_id}\"\
             {na_attr}></saml:SubjectConfirmationData>\
             </saml:SubjectConfirmation></saml:Subject>\
             <saml:Conditions NotBefore=\"{nb}\"{na_attr}>\
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
            bearer = SAML_BEARER_METHOD,
            acs = acs_url,
            request_id = request_id_value,
            na_attr = not_on_or_after_attr,
            aud = audience,
            tenant = tenant,
            region = region,
        );
        let doc = format!(
            "<samlp:Response xmlns:samlp=\"urn:oasis:names:tc:SAML:2.0:protocol\" ID=\"_resp1\" \
             Version=\"2.0\" IssueInstant=\"{nb}\" Destination=\"{acs}\" \
             InResponseTo=\"{request_id}\">{assertion}</samlp:Response>",
            nb = not_before,
            acs = acs_url,
            request_id = request_id_value,
            assertion = assertion,
        );
        finalize(doc, sign)
    }

    fn finalize(doc: String, sign: &dyn Fn(&[u8]) -> Vec<u8>) -> String {
        use sha2::{Digest, Sha256};

        let parsed = parse_xml(&doc).expect("builder doc parses");
        let mut a = Vec::new();
        collect_named(&parsed, &base_scope(), SAML_NS, "Assertion", &mut a);
        let (assertion, inherited) = a.remove(0);
        let mut c = String::new();
        canonicalize(assertion, &inherited, &NsScope::new(), true, &mut c);
        let digest = Sha256::digest(c.as_bytes()).to_vec();
        let doc = doc.replace("@@DIGEST@@", &TB64.encode(&digest));

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
        SamlVerifier::new(config(anchors), ReplayGuard::new())
            .with_request_binding(SamlRequestBinding::new(ACS, REQUEST_ID))
            .with_clock(|| NOW)
    }

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

    #[test]
    fn positive_rsa_sha256_verifies_and_yields_trust_rooted_assertion() {
        let (v, doc) = rsa_signed("_assertion_1");
        let a = v
            .verify(&cred(doc))
            .expect("a correctly-signed RSA-SHA256 assertion must verify");
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
            "_ec_1",
            ISSUER,
            "bob@acme.example",
            "acme",
            "eu-west",
            NB,
            NA,
            SP,
            ECDSA_SHA256,
            &|m| signer.sign(m),
        );
        let a = v
            .verify(&cred(doc))
            .expect("a correctly-signed ECDSA-SHA256 assertion must verify");
        assert_eq!(a.tenant, TenantId("acme".into()));
        assert_eq!(a.subject_key, "bob@acme.example");
    }

    #[test]
    fn positive_region_falls_back_to_configured_idp_binding() {
        let signer = RsaSigner::generate();
        let v = SamlVerifier::new(
            config(vec![signer.jwk()]).with_region_default("ap-south"),
            ReplayGuard::new(),
        )
            .with_request_binding(SamlRequestBinding::new(ACS, REQUEST_ID))
            .with_clock(|| NOW);
        let raw = format!(
            "<samlp:Response xmlns:samlp=\"urn:oasis:names:tc:SAML:2.0:protocol\" ID=\"_resp2\" \
             Version=\"2.0\" IssueInstant=\"{NB}\" Destination=\"{ACS}\" \
             InResponseTo=\"{REQUEST_ID}\">\
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
             <saml:Subject><saml:NameID>carol@acme.example</saml:NameID>\
             <saml:SubjectConfirmation Method=\"{SAML_BEARER_METHOD}\">\
             <saml:SubjectConfirmationData Recipient=\"{ACS}\" InResponseTo=\"{REQUEST_ID}\" \
             NotOnOrAfter=\"{NA}\"></saml:SubjectConfirmationData>\
             </saml:SubjectConfirmation></saml:Subject>\
             <saml:Conditions NotBefore=\"{NB}\" NotOnOrAfter=\"{NA}\">\
             <saml:AudienceRestriction><saml:Audience>{SP}</saml:Audience></saml:AudienceRestriction>\
             </saml:Conditions>\
             <saml:AttributeStatement>\
             <saml:Attribute Name=\"tenant\"><saml:AttributeValue>acme</saml:AttributeValue></saml:Attribute>\
             </saml:AttributeStatement></saml:Assertion></samlp:Response>",
        );
        let doc = finalize(raw, &|m| signer.sign(m));
        let a = v
            .verify(&cred(doc))
            .expect("region-less assertion verifies with the fallback region");
        assert_eq!(
            a.region,
            Region("ap-south".into()),
            "region from the configured IdP binding"
        );
        assert_eq!(a.tenant, TenantId("acme".into()));
    }

    #[test]
    fn verifier_without_server_request_binding_fails_closed() {
        let signer = RsaSigner::generate();
        let verifier = SamlVerifier::new(config(vec![signer.jwk()]), ReplayGuard::new())
            .with_clock(|| NOW);
        let doc = build_doc(
            "_missing_binding",
            ISSUER,
            "alice@acme.example",
            "acme",
            "eu-west",
            NB,
            NA,
            SP,
            RSA_SHA256,
            &|message| signer.sign(message),
        );
        let error = verifier.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&error, AuthzError::FailClosed(message) if message.contains("request binding")),
            "a verifier without server-held request state must fail closed, got {error:?}"
        );
    }

    #[test]
    fn response_destination_must_match_the_bound_acs() {
        let (verifier, doc) = rsa_signed("_wrong_destination");
        let attack = doc.replacen(
            &format!("Destination=\"{ACS}\""),
            "Destination=\"https://evil.example/acs\"",
            1,
        );
        let error = verifier.verify(&cred(attack)).unwrap_err();
        assert!(
            matches!(&error, AuthzError::FailClosed(message) if message.contains("Destination")),
            "a response routed to another ACS must be refused, got {error:?}"
        );
    }

    #[test]
    fn signed_recipient_and_request_id_must_match_the_login_transaction() {
        let signer = RsaSigner::generate();
        let verifier = verifier(vec![signer.jwk()]);
        let wrong_acs = "https://evil.example/acs";
        let wrong_recipient = build_doc_for_binding(
            "_wrong_recipient",
            ISSUER,
            "alice@acme.example",
            "acme",
            "eu-west",
            NB,
            NA,
            SP,
            wrong_acs,
            REQUEST_ID,
            RSA_SHA256,
            &|message| signer.sign(message),
        )
        .replacen(
            &format!("Destination=\"{wrong_acs}\""),
            &format!("Destination=\"{ACS}\""),
            1,
        );
        let error = verifier.verify(&cred(wrong_recipient)).unwrap_err();
        assert!(
            matches!(&error, AuthzError::FailClosed(message) if message.contains("Recipient")),
            "a signed recipient for another ACS must be refused, got {error:?}"
        );

        let wrong_request_id = "_other_authn_request";
        let wrong_response = build_doc_for_binding(
            "_wrong_request",
            ISSUER,
            "alice@acme.example",
            "acme",
            "eu-west",
            NB,
            NA,
            SP,
            ACS,
            wrong_request_id,
            RSA_SHA256,
            &|message| signer.sign(message),
        )
        .replacen(
            &format!("InResponseTo=\"{wrong_request_id}\""),
            &format!("InResponseTo=\"{REQUEST_ID}\""),
            1,
        );
        let error = verifier.verify(&cred(wrong_response)).unwrap_err();
        assert!(
            matches!(&error, AuthzError::FailClosed(message) if message.contains("SubjectConfirmationData InResponseTo")),
            "a signed response tied to another AuthnRequest must be refused, got {error:?}"
        );
    }

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

    #[test]
    fn xsw_2_forged_assertion_wrapping_the_signed_one_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_1");
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

    #[test]
    fn xsw_4_duplicate_id_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_1");
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

    #[test]
    fn xsw_signature_not_enveloped_child_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_1");
        let start = doc.find("<ds:Signature ").unwrap();
        let end = doc.find("</ds:Signature>").unwrap() + "</ds:Signature>".len();
        let sig = doc[start..end].to_string();
        let without = format!("{}{}", &doc[..start], &doc[end..]);
        let attack = without.replace("</saml:Assertion>", &format!("</saml:Assertion>{sig}"));
        let err = v.verify(&cred(attack)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("schema-position") || m.contains("enveloped")),
            "a non-enveloped signature must be refused, got {err:?}"
        );
    }

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

    #[test]
    fn signature_by_non_anchored_key_is_rejected() {
        let victim = RsaSigner::generate();
        let attacker = RsaSigner::generate();
        let v = verifier(vec![victim.jwk()]);
        let doc = build_doc(
            "_assertion_1",
            ISSUER,
            "attacker@globex.evil",
            "globex",
            "eu-west",
            NB,
            NA,
            SP,
            RSA_SHA256,
            &|m| attacker.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("verification failed")),
            "a signature by a non-anchored key must be refused, got {err:?}"
        );
    }

    #[test]
    fn sha1_signature_method_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            "_assertion_1",
            ISSUER,
            "alice@acme.example",
            "acme",
            "eu-west",
            NB,
            NA,
            SP,
            "http://www.w3.org/2000/09/xmldsig#rsa-sha1",
            &|m| signer.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("SHA-1") || m.contains("rsa-sha1")),
            "a SHA-1 signature method must be refused, got {err:?}"
        );
    }

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

    #[test]
    fn expired_assertion_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            "_assertion_1",
            ISSUER,
            "alice@acme.example",
            "acme",
            "eu-west",
            "2000-01-01T00:00:00Z",
            "2000-01-02T00:00:00Z",
            SP,
            RSA_SHA256,
            &|m| signer.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("expired")),
            "an expired assertion must be refused, got {err:?}"
        );
    }

    #[test]
    fn assertion_without_not_on_or_after_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            "_assertion_without_expiry",
            ISSUER,
            "alice@acme.example",
            "acme",
            "eu-west",
            NB,
            "",
            SP,
            RSA_SHA256,
            &|m| signer.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("Conditions has no NotOnOrAfter")),
            "an assertion without a finite expiry must be refused, got {err:?}"
        );
    }

    #[test]
    fn not_yet_valid_assertion_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            "_assertion_1",
            ISSUER,
            "alice@acme.example",
            "acme",
            "eu-west",
            "2099-01-01T00:00:00Z",
            "2099-01-02T00:00:00Z",
            SP,
            RSA_SHA256,
            &|m| signer.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("not yet valid")),
            "a not-yet-valid assertion must be refused, got {err:?}"
        );
    }

    #[test]
    fn wrong_audience_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            "_assertion_1",
            ISSUER,
            "alice@acme.example",
            "acme",
            "eu-west",
            NB,
            NA,
            "https://some-other-sp.example.com",
            RSA_SHA256,
            &|m| signer.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("audience")),
            "a wrong-audience assertion must be refused, got {err:?}"
        );
    }

    #[test]
    fn wrong_issuer_is_rejected() {
        let signer = RsaSigner::generate();
        let v = verifier(vec![signer.jwk()]);
        let doc = build_doc(
            "_assertion_1",
            "https://evil-idp.example.com",
            "alice@acme.example",
            "acme",
            "eu-west",
            NB,
            NA,
            SP,
            RSA_SHA256,
            &|m| signer.sign(m),
        );
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("issuer")),
            "a wrong-issuer assertion must be refused, got {err:?}"
        );
    }

    #[test]
    fn replayed_assertion_is_rejected() {
        let (v, doc) = rsa_signed("_assertion_replay");
        v.verify(&cred(doc.clone()))
            .expect("first presentation verifies");
        let err = v.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("replay")),
            "a replayed assertion must be refused, got {err:?}"
        );
    }

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
            "<root>".to_string() + &"<x>".repeat(5000),
            "\u{0}\u{1}\u{2}garbage".into(),
            "<a xmlns:ds=\"\">".into(),
            "<ds:Signature></ds:Signature>".into(),
            "<samlp:Response xmlns:samlp=\"urn:oasis:names:tc:SAML:2.0:protocol\"></samlp:Response>".into(),
        ];
        for (i, bad) in cases.iter().enumerate() {
            let r = v.verify(&cred(bad.clone()));
            assert!(
                r.is_err(),
                "garbage case {i} must be refused (and must not panic): {bad:?}"
            );
        }
    }

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
        let (v2, ok) = rsa_signed("_assertion_depth_ok");
        assert!(
            v2.verify(&cred(ok)).is_ok(),
            "a shallow assertion must still verify"
        );
    }

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

    #[test]
    fn dispatch_routes_saml_to_real_verifier() {
        let (v, doc) = rsa_signed("_assertion_dispatch");
        let dispatch = SchemeDispatchVerifier::new(Arc::new(StructuralVerifier::new()))
            .route(scheme::SAML, Arc::new(v));

        let a = dispatch
            .verify(&cred(doc))
            .expect("real SAML verifies via the dispatcher");
        assert_eq!(a.tenant, TenantId("acme".into()));
        assert_eq!(a.scheme, scheme::SAML);

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

    #[test]
    fn shared_replay_guard_blocks_cross_handle_replay() {
        let signer = RsaSigner::generate();
        let guard = ReplayGuard::new();
        let v1 = SamlVerifier::new(config(vec![signer.jwk()]), guard.clone())
            .with_request_binding(SamlRequestBinding::new(ACS, REQUEST_ID))
            .with_clock(|| NOW);
        let v2 = SamlVerifier::new(config(vec![signer.jwk()]), guard)
            .with_request_binding(SamlRequestBinding::new(ACS, REQUEST_ID))
            .with_clock(|| NOW);
        let doc = build_doc(
            "_assertion_shared",
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
        v1.verify(&cred(doc.clone()))
            .expect("first handle verifies");
        let err = v2.verify(&cred(doc)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("replay")),
            "the shared guard must block a cross-handle replay, got {err:?}"
        );
    }
}
