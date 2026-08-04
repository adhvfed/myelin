//! Structural source pins over the `gvisor` modules: properties a behavioural test cannot
//! reach, because they assert that NO code path exists.

use super::*;

// =============================================================================================
// CT-007 phase-credential generations: SOURCE PINS.
//
// These read this module's own source text. A behavioural test can only prove that the paths it
// exercises are gated; a source pin proves that NO path exists — which is exactly the property
// "no raw preparation spawn is reachable with an immediate permit in the V2 API" asserts.
// =============================================================================================

/// Every production source file the `gvisor` module's body was split across. Before the split these
/// pins read a single `gvisor.rs`; the text they scan is unchanged, it just arrives in pieces.
const PRODUCTION_SOURCES: &[&str] = &[
    include_str!("../gvisor.rs"),
    include_str!("preflight.rs"),
    include_str!("oci_config.rs"),
    include_str!("workspace_lease.rs"),
    include_str!("backend.rs"),
    include_str!("checkout_launch.rs"),
    include_str!("run.rs"),
    include_str!("teardown.rs"),
    include_str!("output_capture.rs"),
    include_str!("rootfs.rs"),
    include_str!("git_wire_run.rs"),
    include_str!("checkout_transport.rs"),
    include_str!("checkout_preparation.rs"),
];

/// The dedicated capsule submodule's source (CT-007 5b.3-6a, Sol's r4): the capsule types + the
/// reshaped Hop B entry moved here so module privacy enforces field inseparability.
const CHECKOUT_RUNTIME_SOURCE: &str = include_str!("checkout_runtime.rs");

/// One source file's PRODUCTION text: everything before its top-level `#[cfg(test)] mod tests`. A
/// whole-file `contains` check would otherwise match the assertion strings in the tests themselves --
/// a source pin that reads its own crate must exclude the test regions or it asserts against itself.
fn production_of(source: &'static str) -> &'static str {
    let end = source
        .find("\n#[cfg(test)]\nmod tests {")
        .or_else(|| source.find(TEST_MODULE_BANNER));
    match end {
        Some(end) => &source[..end],
        None => source,
    }
}

/// `gvisor.rs` has no test module of its own to split on: its `#[cfg(test)]`/`test-support` module
/// declarations sit below this banner instead. Before the split those declarations were written
/// after the top-level test module, which is what kept them out of the scanned text.
const TEST_MODULE_BANNER: &str = "\n// ══════ the test and test-support modules ══════";

/// The whole `gvisor` module's PRODUCTION source.
pub(super) fn production_source() -> String {
    PRODUCTION_SOURCES
        .iter()
        .map(|source| production_of(source))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of a named `fn`/`pub(crate) fn`, from its signature to the next top-level `}` at
/// column 0 -- enough to scope a source pin to one function without pulling in its neighbours.
fn source_in(source: &'static str, function_signature: &str) -> Option<&'static str> {
    let start = source.find(function_signature)?;
    let rest = &source[start..];
    let end = rest.find("\n}\n")?;
    Some(&rest[..end])
}

/// [`source_of`] against an arbitrary source string (used for the capsule submodule).
fn source_of_in(source: &'static str, function_signature: &str) -> &'static str {
    source_in(source, function_signature).unwrap_or_else(|| panic!("`{function_signature}` exists"))
}

fn source_of(function_signature: &str) -> &'static str {
    PRODUCTION_SOURCES
        .iter()
        .map(|source| production_of(source))
        .find_map(|source| source_in(source, function_signature))
        .unwrap_or_else(|| panic!("`{function_signature}` exists in this module"))
}

/// **In the V2 API, neither preparation spawn can construct its own permit — and the V2 entry
/// points offer no legacy option at the TYPE level.** These pins are the structural complement
/// to the behavioural tests: a behavioural test proves the paths it exercises are gated, a
/// source pin proves no ungated path EXISTS.
#[test]
fn no_v2_preparation_spawn_can_mint_its_own_immediate_permit() {
    let hop = source_of("fn run_one_git_wire_hop_within_parent_attempt(");
    assert!(
        !hop.contains("LaunchPermit::immediate()"),
        "the git-wire hop runner must consume the permit it is handed, never mint one"
    );
    assert!(
        hop.contains("permit: LaunchPermit"),
        "the git-wire hop runner takes its permit as an argument"
    );

    // Hop B: the shared body takes its permit; only the LEGACY entry point mints an immediate
    // one; the V2 entry point resolves it by CONSUMING a PhaseAuthorization.
    let inner = source_of("fn run_checkout_preparation_inner(");
    assert!(
        inner.contains("launch_permit: LaunchPermit") && inner.contains("Some(launch_permit)"),
        "Hop B's body consumes the permit it is handed, never mints one"
    );
    assert!(
        !inner.contains("LaunchPermit::immediate()"),
        "Hop B's body must never mint a permit inline"
    );
    let legacy_preparation = source_of("pub(crate) fn run_checkout_preparation(");
    assert_eq!(
        legacy_preparation
            .matches("LaunchPermit::immediate()")
            .count(),
        1,
        "the LEGACY Hop B entry point is the one place an immediate preparation permit exists"
    );
    // The V2 Hop B entry now lives in the dedicated `checkout_runtime` submodule (Sol's r4).
    let v2_preparation = source_of_in(
        CHECKOUT_RUNTIME_SOURCE,
        "pub(crate) fn run_checkout_preparation_v2(",
    );
    assert!(
        !v2_preparation.contains("LaunchPermit::immediate()")
            && !v2_preparation.contains("CheckoutAuthorizationProof"),
        "the V2 Hop B entry point can neither mint an immediate permit nor accept a legacy proof"
    );
    assert!(
        v2_preparation.contains("authorization: PhaseAuthorization"),
        "the V2 Hop B entry point takes the fused, non-constructible authorization"
    );
    let resolve = source_of("fn resolve_checkout_preparation_permit(");
    assert!(
        !resolve.contains("LaunchPermit::immediate()")
            && resolve.contains(
                "into_preparation_permit_for_scope(run_token, checkout_scope, expected_commit)"
            ),
        "the V2 permit is reachable ONLY by consuming the authorization through its own checks, \
         bound to the capsule's FULL scope (5b.3-6a blocker 1)"
    );

    // Hop A: the module-private authority enum means the V2 entry point cannot select a legacy
    // arm, and the legacy immediate permits live only on the legacy arm of the shared body.
    let production = production_source();
    assert!(
        production.contains("\nenum TransportAuthority<'a> {")
            && !production.contains("pub(crate) enum TransportAuthority")
            && !production.contains("pub enum TransportAuthority"),
        "the transport authority enum is MODULE-PRIVATE: no other module can select an arm"
    );
    let v2_transport = source_of("pub(crate) fn fetch_checkout_pack_within_parent_attempt_v2(");
    assert!(
        !v2_transport.contains("CheckoutAuthorizationProof")
            && !v2_transport.contains("LaunchPermit"),
        "the V2 Hop A entry point accepts neither a legacy proof nor a bare permit"
    );
    assert!(
        v2_transport.contains("advertise: PhaseAuthorization")
            && v2_transport.contains("Result<(RunTokenCredential, PhaseAuthorization), HookError>"),
        "the V2 Hop A entry point takes the fused authorization for both legs"
    );
    let inner_transport = source_of("fn fetch_checkout_pack_within_parent_attempt_inner(");
    assert_eq!(
        inner_transport.matches("LaunchPermit::immediate()").count(),
        2,
        "exactly two immediate permits, both on the legacy arm (advertise + fetch)"
    );
    assert!(
        inner_transport.contains("TransportAuthority::LegacyClaimBound { proof } => {")
            && inner_transport.contains("(None, LaunchPermit::immediate(), None)"),
        "the advertise immediate permit belongs to the legacy arm"
    );
    assert!(
        inner_transport.contains("None => (run_token, LaunchPermit::immediate()),"),
        "the fetch immediate permit belongs to the legacy (no fetch provider) arm"
    );
    assert!(
        inner_transport.contains(
            "let permit = advertise\n                .into_transport_permit(\n                    crate::CheckoutPhase::Advertise,"
        ),
        "the V2 advertise leg reaches its permit only by CONSUMING its authorization"
    );
    assert!(
        inner_transport.contains("into_transport_permit(crate::CheckoutPhase::Fetch, &credential, tenant, repo, expected)"),
        "the V2 fetch leg reaches its permit only by consuming its authorization, checked \
         against the credential the SAME provider returned"
    );
}

/// **The fused authorization cannot be taken apart.** `PhaseAuthorization` has no public
/// constructor, is not `Clone`, and its permit field is only ever moved out by a consuming
/// `into_*_permit` that runs the provenance checks first.
#[test]
fn the_phase_authorization_is_structurally_inseparable() {
    const AUTHORIZATION_SOURCE: &str = include_str!("../checkout_authorization.rs");
    assert!(
        AUTHORIZATION_SOURCE.contains("pub(crate) struct PhaseAuthorization {")
            && AUTHORIZATION_SOURCE.contains("    permit: LaunchPermit,"),
        "the permit is a PRIVATE field of the fused authorization"
    );
    let declaration = AUTHORIZATION_SOURCE
        .split("pub(crate) struct PhaseAuthorization {")
        .next()
        .expect("the declaration exists");
    // Whatever attribute block immediately precedes the struct must not derive Clone/Copy.
    let attributes = declaration
        .rsplit("\n\n")
        .next()
        .expect("there is an attribute block");
    assert!(
        !attributes.contains("derive(") || !attributes.contains("Clone"),
        "the authorization is deliberately NOT Clone: it cannot be duplicated across legs"
    );
    assert!(
        !AUTHORIZATION_SOURCE.contains("impl Clone for PhaseAuthorization"),
        "the authorization must not hand-implement Clone either"
    );
    // Round-2 minor 3: pin EVERY access of `self.permit`, not just the two `Ok(self.permit)`
    // returns. A future `pub(crate) fn leak(self) -> LaunchPermit { self.permit }` would add a
    // third `self.permit` and fail here even though it changes neither the `Ok(self.permit)`
    // count, the construction count, nor the Clone checks.
    assert_eq!(
        AUTHORIZATION_SOURCE.matches("self.permit").count(),
        2,
        "the permit field is TOUCHED in exactly two places: the two consuming into_*_permit \
         methods (transport, and the 5b.3-6a full-scope preparation — the commit-only preparation \
         permit was removed in r2). A new permit-exposing method would raise this count."
    );
    assert_eq!(
        AUTHORIZATION_SOURCE.matches("Ok(self.permit)").count(),
        2,
        "the permit escapes only through the two consuming into_*_permit methods"
    );
    // Pin the COMPLETE method surface of `impl PhaseAuthorization` — an exact set. Any new
    // method (permit-exposing or otherwise) fails until it is reviewed and added here.
    //
    // Round-3 minor: the parser must recognize a method under ANY visibility/modifier form, or a
    // leak could hide behind a spelling the parser skips. The exact shapes defended against:
    //   pub(super) async fn leak(self) -> LaunchPermit { let Self { permit, .. } = self; permit }
    //   pub(in crate::foo) const unsafe fn leak(self) -> LaunchPermit { self.permit }
    // The (a) method-surface enumeration below strips every `pub`/`pub(..)` visibility and every
    // `async`/`const`/`unsafe`/`extern` modifier before reading the `fn` name, so ANY new method
    // (regardless of spelling) enters the parsed set and breaks the exact-set assertion; the (b)
    // destructuring guard forbids `Self {` / `PhaseAuthorization {` binding patterns inside the
    // impl, closing the "move the field out by pattern" route the `self.permit` counter misses.
    let impl_block = {
        let start = AUTHORIZATION_SOURCE
            .find("\nimpl PhaseAuthorization {")
            .expect("the impl block exists");
        let rest = &AUTHORIZATION_SOURCE[start + 1..];
        let end = rest.find("\n}\n").expect("the impl block closes");
        &rest[..end]
    };
    // Return the `fn` name of a method declaration under any visibility/modifier chain.
    fn method_name(line: &str) -> Option<&str> {
        let mut rest = line.trim_start();
        // One visibility token: `pub`, `pub(crate)`, `pub(super)`, `pub(in path)`.
        if let Some(after_pub) = rest.strip_prefix("pub") {
            if let Some(inner) = after_pub.strip_prefix('(') {
                let close = inner.find(')')?;
                rest = inner[close + 1..].trim_start();
            } else if after_pub.starts_with(char::is_whitespace) {
                rest = after_pub.trim_start();
            }
            // else: `pub` was a prefix of some other identifier — leave `rest` as-is; it will
            // fail the `fn ` check below and be ignored.
        }
        // Any combination of `async`/`const`/`unsafe`/`extern "ABI"` modifiers, in any order.
        loop {
            let mut advanced = false;
            for keyword in ["async", "const", "unsafe", "extern"] {
                if let Some(after) = rest.strip_prefix(keyword) {
                    if after.starts_with(char::is_whitespace) || after.starts_with('"') {
                        rest = after.trim_start();
                        if keyword == "extern" {
                            if let Some(abi) = rest.strip_prefix('"') {
                                if let Some(end) = abi.find('"') {
                                    rest = abi[end + 1..].trim_start();
                                }
                            }
                        }
                        advanced = true;
                    }
                }
            }
            if !advanced {
                break;
            }
        }
        let after_fn = rest.strip_prefix("fn ")?;
        let name = after_fn.split(['(', '<', ' ']).next()?.trim();
        (!name.is_empty()).then_some(name)
    }
    let mut methods: Vec<&str> = impl_block.lines().filter_map(method_name).collect();
    methods.sort_unstable();
    assert_eq!(
        methods,
        vec![
            "generation_id",
            "into_preparation_permit_for_scope",
            "into_transport_permit",
            "phase",
            "run_token_jti",
            "verify_provenance",
        ],
        "the PhaseAuthorization method surface changed — any new method (under ANY visibility or \
         modifier) that could move or expose `self.permit` (or destructure self) must be \
         reviewed and pinned here"
    );
    // (b) Destructuring is the other route to a private field. Forbid BOTH binding spellings
    // (`let Self { permit, .. } = self` and `let PhaseAuthorization { .. } = self`) ANYWHERE
    // inside the inherent impl block.
    assert_eq!(
        impl_block.matches("Self {").count(),
        0,
        "no `Self {{ .. }}` destructuring pattern inside impl PhaseAuthorization may bind the \
         private permit"
    );
    assert_eq!(
        impl_block.matches("PhaseAuthorization {").count(),
        1,
        "the only `PhaseAuthorization {{` inside the impl block is its own header — never a \
         destructuring pattern that could pull the permit out"
    );
    // Belt-and-braces global count too (struct decl, Debug header, inherent-impl header, and the
    // ONE construction site in `RunnerHooks::authorize_checkout_phase`).
    assert_eq!(
        AUTHORIZATION_SOURCE.matches("PhaseAuthorization {").count(),
        4,
        "exactly: the struct decl, the Debug impl header, the inherent impl header, and the one \
         construction site — no destructuring pattern reaches the private permit"
    );
    assert_eq!(
        AUTHORIZATION_SOURCE
            .matches("self.verify_provenance(")
            .count(),
        2,
        "every consumption runs the phase/JTI/generation provenance check first"
    );
    assert!(
        AUTHORIZATION_SOURCE.contains("PhaseAuthorization {\n            scope,"),
        "the ONE construction site fuses the scope, retained JTI, phase, generation, and permit"
    );
    assert_eq!(
        AUTHORIZATION_SOURCE.matches("permit,\n        })").count(),
        1,
        "there is exactly ONE construction site for the fused authorization"
    );
}

/// **The checkout-runtime submodule's SHAPE is audited CLOSED-WORLD (syn AST).** (CT-007 slice
/// 5b.3-6a, Sol's r4/r5.) The capsule types + their five approved accessors live in the dedicated
/// private submodule `checkout_runtime`, so **Rust's own module privacy** — not this test — forbids
/// any code OUTSIDE the module (sibling, free fn, macro expansion, descendant module) from NAMING
/// the inner fields; the compile-error bites-proofs fail to COMPILE with a privacy error, not a
/// test. But code INSIDE the module can still EXPORT a leak (Sol's r5: `pub(crate) static LEAK:
/// fn(&Cap)->&OciConfig = |c| &c.workload_cfg;` — the closure's field access is legal inside the
/// owning module, and a parent calls `checkout_runtime::LEAK`). So this test makes the module's
/// export inventory CLOSED-WORLD: parsing `checkout_runtime.rs`, EVERY production (non-`#[cfg(test)]`)
/// top-level item MUST be one of — `use` imports (any number); EXACTLY the two capsule structs (no
/// other struct/enum/union/type/alias); INHERENT impls on ONLY those two types (no trait impl, no
/// impl on another self-type); and ONLY the free fns in `ALLOWED_FREE_FNS`. Any other item kind —
/// `static`, `const`, `trait`, `macro_rules!`/macro invocation, `extern`, `mod`, type alias, union,
/// an extra free fn — FAILS the audit BY NAME. Together with module privacy, this audited inventory
/// is the compile-time guarantee: there is nowhere inside the module to hide a leaking
/// static/const/helper. The audit ALSO checks capsule fields private, no `Clone`/`Copy`, no non-`fn`
/// associated items, and the exact non-private accessor surface (the five entries).
///
/// MULTIPLICITY-EXACT (Sol's r6): the struct-name, free-fn-name, and accessor-surface inventories
/// are SORTED LISTS compared with multiplicity (no dedup, no set-membership). `syn` parses BOTH
/// arms of a `#[cfg]`/`#[cfg(not)]` pair as two items, so a second gated definition of an approved
/// name (which a comment can hide from a literal occurrence pin) makes its list one entry LONGER
/// than the allowlist and FAILS — where a set would have collapsed it to one.
///
/// RESIDUAL — HONEST SCOPE: `syn` does not expand macros. Any macro invocation IN THIS MODULE whose
/// token stream contains a capsule TYPE ident or ANY inner-field ident (all five, incl.
/// `prepared_checkout_evidence`) fails this test, forcing review; and a top-level `macro_rules!`/
/// macro invocation is itself rejected by the closed-world inventory. The remaining unexpanded
/// external/procedural-macro gap is acceptable for this dormant guard (Sol's r4/r5: do not move it
/// to `myelin-lints`).
#[test]
fn the_checkout_runtime_module_shape_is_pinned() {
    const CAPSULES: [&str; 2] = ["AcquiredCheckoutRuntime", "PreparedCheckoutRuntime"];
    const MACRO_REVIEW_IDENTS: [&str; 7] = [
        "AcquiredCheckoutRuntime",
        "PreparedCheckoutRuntime",
        "workload_cfg",
        "enabled_context",
        "session",
        "acquired",
        "prepared_checkout_evidence",
    ];
    // CT-007 slice 5b.3-6c: the standalone `PreparedCheckoutRuntime::bind_workload` synthetic-identity
    // helper was FOLDED into the ONE closed `run_retained_workload` transition — its allowlist entry
    // is REPLACED (still exactly FIVE accessors). This is deliberate audited-API evolution: a caller
    // can request the whole sanctioned workload transition, but can no longer extract or substitute
    // its constituent bind capability.
    const ALLOWLIST: [&str; 5] = [
        "AcquiredCheckoutRuntime::acquire",
        "AcquiredCheckoutRuntime::dispose_checkout_runtime",
        "PreparedCheckoutRuntime::dispose_checkout_runtime",
        "PreparedCheckoutRuntime::run_retained_workload",
        "run_checkout_preparation_v2",
    ];
    // CT-007 slice 5b.3-6c: the EXACT `#[cfg(test)]`-only method inventory. The audit already
    // excludes the whole test-only impl from the production accessor surface; pinning the test set
    // exactly keeps that exception explicit — a new test-only capsule method fails until reviewed.
    // CT-007 slice 5b.3-6c (Sol's finding 6): the exact `#[cfg(test)]`-only capsule method set —
    // the session driver, the into-prepared type-state transition, and the injectable workload
    // execution seam. Pinned exactly so a new test-only capsule method fails until reviewed.
    const TEST_METHODS: [&str; 3] = [
        "drive_session_for_tests",
        "into_prepared_for_tests",
        "run_retained_workload_given",
    ];
    // CT-007 slice 5b.3-6e.1b/6e.2: the EXACT `#[cfg(any(test, feature = "test-support"))]`-only
    // method inventory. The deterministic substituted-execution seam is gated for `test-support`
    // (so the hardware-independent runsc-driver fixture can reach it), NOT `#[cfg(test)]` — so it
    // is recognized as its OWN test-support surface and does NOT count against the FIVE-entry
    // production accessor `ALLOWLIST`. 6e.2 SPLIT the single seam into its Hop B half (on
    // `AcquiredCheckoutRuntime`, returning the fused prepared capsule) and its workload half (on
    // `PreparedCheckoutRuntime`, driving the REAL `run_retained_workload_inner`), so the ruling-(A)
    // workload leg runs the real authority/settle path. Pinned exactly so a new test-support
    // capsule method fails until reviewed.
    const TEST_SUPPORT_METHODS: [&str; 2] = [
        "substituted_hop_b_for_test_support",
        "substituted_workload_for_test_support",
    ];
    // Closed-world (Sol's r5): the EXACT set of free functions permitted at module top level. Any
    // other free fn — even a private helper — is a violation until reviewed and added here.
    const ALLOWED_FREE_FNS: [&str; 1] = ["run_checkout_preparation_v2"];

    fn type_last_ident(ty: &syn::Type) -> Option<String> {
        match ty {
            syn::Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.to_string()),
            syn::Type::Reference(r) => type_last_ident(&r.elem),
            _ => None,
        }
    }
    fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            let mut hit = false;
            if a.path().is_ident("cfg") {
                let _ = a.parse_nested_meta(|meta| {
                    if meta.path.is_ident("test") {
                        hit = true;
                    }
                    Ok(())
                });
            }
            hit
        })
    }
    fn is_private(vis: &syn::Visibility) -> bool {
        matches!(vis, syn::Visibility::Inherited)
    }
    /// Whether an item is gated `#[cfg(any(test, feature = "test-support"))]` (or
    /// `#[cfg(feature = "test-support")]`) — the test-support EXECUTION seam, distinct from the
    /// `#[cfg(test)]`-only driver impl. Detected by the `test-support` feature token inside a
    /// `cfg(...)` attribute; recognized as its own inventory so it never counts against the
    /// five-entry production accessor surface.
    fn is_cfg_test_support(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            a.path().is_ident("cfg")
                && matches!(&a.meta, syn::Meta::List(list) if list.tokens.to_string().contains("test-support"))
        })
    }
    /// A human name for a forbidden top-level item kind (for the closed-world violation message).
    fn describe_item(item: &syn::Item) -> String {
        match item {
            syn::Item::Const(c) => format!("const `{}`", c.ident),
            syn::Item::Static(s) => format!("static `{}`", s.ident),
            syn::Item::Trait(t) => format!("trait `{}`", t.ident),
            syn::Item::TraitAlias(t) => format!("trait alias `{}`", t.ident),
            syn::Item::Type(t) => format!("type alias `{}`", t.ident),
            syn::Item::Enum(e) => format!("enum `{}`", e.ident),
            syn::Item::Union(u) => format!("union `{}`", u.ident),
            syn::Item::Mod(m) => format!("module `{}`", m.ident),
            syn::Item::Macro(m) => format!(
                "macro `{}!`",
                m.mac
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default()
            ),
            syn::Item::ForeignMod(_) => "an extern block".to_string(),
            syn::Item::ExternCrate(e) => format!("extern crate `{}`", e.ident),
            _ => "an unrecognized item kind".to_string(),
        }
    }
    fn macro_mentions(ts: &proc_macro2::TokenStream, needles: &[&str]) -> Option<String> {
        for tt in ts.clone() {
            match tt {
                proc_macro2::TokenTree::Ident(id) => {
                    let s = id.to_string();
                    if needles.contains(&s.as_str()) {
                        return Some(s);
                    }
                }
                proc_macro2::TokenTree::Group(g) => {
                    if let Some(h) = macro_mentions(&g.stream(), needles) {
                        return Some(h);
                    }
                }
                _ => {}
            }
        }
        None
    }

    let file = syn::parse_file(production_of(CHECKOUT_RUNTIME_SOURCE))
        .expect("checkout_runtime.rs parses as a File");
    let mut violations: Vec<String> = Vec::new();
    // MULTIPLICITY-EXACT inventories (Sol's r6): NO dedup. `syn` parses BOTH arms of a
    // `#[cfg]`/`#[cfg(not)]` pair as two separate items (it never evaluates cfg), so two gated
    // definitions of an approved name land as TWO list entries — making the list LONGER than its
    // allowlist and failing, rather than collapsing into one set entry.
    let mut accessor_surface: Vec<String> = Vec::new();
    let mut test_method_names: Vec<String> = Vec::new();
    let mut test_support_method_names: Vec<String> = Vec::new();
    let mut free_fn_names: Vec<String> = Vec::new();
    let mut struct_names: Vec<String> = Vec::new();

    // Macro scan over the whole module (syn::visit reaches nested macros too).
    {
        use syn::visit::Visit;
        struct MacroScan<'a> {
            violations: &'a mut Vec<String>,
        }
        impl<'ast> Visit<'ast> for MacroScan<'_> {
            fn visit_macro(&mut self, node: &'ast syn::Macro) {
                if let Some(hit) = macro_mentions(&node.tokens, &MACRO_REVIEW_IDENTS) {
                    let path = node
                        .path
                        .segments
                        .last()
                        .map(|s| s.ident.to_string())
                        .unwrap_or_default();
                    self.violations.push(format!(
                        "macro `{path}!` in the capsule module mentions `{hit}` — `syn` cannot \
                         expand it, so it must be reviewed for a capsule-field leak"
                    ));
                }
                syn::visit::visit_macro(self, node);
            }
        }
        MacroScan {
            violations: &mut violations,
        }
        .visit_file(&file);
    }

    // CLOSED-WORLD inventory (Sol's r5): module privacy stops OUTSIDE code from naming the fields,
    // but code INSIDE the module can still export a leak (e.g. `pub(crate) static LEAK: fn(&Cap)
    // -> &OciConfig = |c| &c.workload_cfg;` — the closure's field access is legal inside the owning
    // module, and a parent then calls `checkout_runtime::LEAK`). So EVERY production top-level item
    // must be one of an exact whitelist; anything else fails the audit by name. Together with
    // module privacy this makes the audited export inventory the compile-time guarantee.
    for item in &file.items {
        match item {
            // ALLOWED: any number of `use` imports.
            syn::Item::Use(_) => {}

            // ALLOWED: EXACTLY the two capsule structs (no other struct/enum/union/type/alias).
            syn::Item::Struct(s) => {
                let name = s.ident.to_string();
                struct_names.push(name.clone());
                if !CAPSULES.contains(&name.as_str()) {
                    violations.push(format!(
                        "unexpected struct `{name}` — only the two capsule structs are permitted"
                    ));
                    continue;
                }
                for f in &s.fields {
                    if !is_private(&f.vis) {
                        violations.push(format!("{name}: a field is not private (pub/pub(..))"));
                    }
                }
                for attr in &s.attrs {
                    if attr.path().is_ident("derive") {
                        let _ = attr.parse_nested_meta(|meta| {
                            if meta.path.is_ident("Clone") || meta.path.is_ident("Copy") {
                                violations.push(format!("{name} derives Clone/Copy"));
                            }
                            Ok(())
                        });
                    }
                }
            }

            // ALLOWED: only the EXACT free-fn name set (even a private helper is rejected until
            // reviewed and added to `ALLOWED_FREE_FNS`).
            syn::Item::Fn(f) => {
                let name = f.sig.ident.to_string();
                free_fn_names.push(name.clone());
                if !ALLOWED_FREE_FNS.contains(&name.as_str()) {
                    violations.push(format!(
                        "unexpected free fn `{name}` — the capsule module permits only \
                         {ALLOWED_FREE_FNS:?} at top level"
                    ));
                } else if !is_cfg_test(&f.attrs) && !is_private(&f.vis) {
                    accessor_surface.push(name);
                }
            }

            // ALLOWED: INHERENT impls on the two capsule types ONLY (incl. the `#[cfg(test)]`
            // driver impl). Any trait impl, or any impl on another self-type, is rejected.
            syn::Item::Impl(im) => {
                if im.trait_.is_some() {
                    violations.push(format!(
                        "trait impl on `{}` — forbidden (a `match self` could hand out the \
                         inner fields)",
                        type_last_ident(&im.self_ty).as_deref().unwrap_or("<type>")
                    ));
                    continue;
                }
                match type_last_ident(&im.self_ty).as_deref() {
                    Some(name) if CAPSULES.contains(&name) => {
                        let test_only = is_cfg_test(&im.attrs);
                        let test_support_only = is_cfg_test_support(&im.attrs);
                        for it in &im.items {
                            match it {
                                syn::ImplItem::Fn(m) => {
                                    if test_only {
                                        // CT-007 slice 5b.3-6c: the whole `#[cfg(test)]` impl is
                                        // excluded from the production accessor surface — but its
                                        // method set is pinned EXACTLY (see `TEST_METHODS`).
                                        test_method_names.push(m.sig.ident.to_string());
                                    } else if test_support_only {
                                        // CT-007 slice 5b.3-6e.1b: the whole `#[cfg(any(test,
                                        // feature = "test-support"))]` impl is likewise excluded
                                        // from the production accessor surface, inventoried
                                        // separately (see `TEST_SUPPORT_METHODS`) so the FIVE-entry
                                        // production surface is unchanged.
                                        test_support_method_names.push(m.sig.ident.to_string());
                                    } else if !is_private(&m.vis) {
                                        accessor_surface.push(format!("{name}::{}", m.sig.ident));
                                    }
                                }
                                _ => violations.push(format!(
                                    "non-fn associated item in inherent impl of `{name}` — a \
                                     const/type could hand out an inner field"
                                )),
                            }
                        }
                    }
                    other => violations.push(format!(
                        "inherent impl on `{}` — only the two capsule types may be `impl`ed in \
                         this module",
                        other.unwrap_or("<non-path type>")
                    )),
                }
            }

            // EVERYTHING ELSE is forbidden: static/const/trait/macro/extern/mod/type-alias/union/…
            // — any of which could export a closure/const/helper that legally reads a private
            // field. This is the terminal closed-world guarantee.
            other => violations.push(format!(
                "forbidden top-level item in the capsule module: {} — its production surface is \
                 closed-world (only `use`, the two capsule structs, their inherent impls, and \
                 run_checkout_preparation_v2)",
                describe_item(other)
            )),
        }
    }

    assert!(
        violations.is_empty(),
        "checkout_runtime module shape violated: {violations:#?}"
    );

    // MULTIPLICITY-EXACT assertions (sorted lists, NO dedup): a second `#[cfg]`-gated definition
    // of an approved name makes its list one entry LONGER than the allowlist and fails here, even
    // though a comment can defeat the literal occurrence pin and a `set` would collapse it.
    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }
    assert_eq!(
        sorted(struct_names),
        sorted(CAPSULES.iter().map(|s| s.to_string()).collect()),
        "the capsule struct-name list changed — EXACTLY the two capsule structs, no duplicate \
         (e.g. a second cfg-gated struct reusing a capsule name)"
    );
    assert_eq!(
        sorted(free_fn_names),
        sorted(ALLOWED_FREE_FNS.iter().map(|s| s.to_string()).collect()),
        "the top-level free-fn list changed — EXACTLY run_checkout_preparation_v2, counted with \
         multiplicity, so a second cfg-gated definition of that name fails"
    );
    assert_eq!(
        sorted(accessor_surface),
        sorted(ALLOWLIST.iter().map(|s| s.to_string()).collect()),
        "the checkout_runtime module's non-private accessor surface changed — every accessor must \
         be an explicitly-reviewed capsule entry, counted with multiplicity"
    );
    assert_eq!(
        sorted(test_method_names),
        sorted(TEST_METHODS.iter().map(|s| s.to_string()).collect()),
        "the checkout_runtime module's `#[cfg(test)]`-only method set changed — every test-only \
         capsule method (the session driver, the into_prepared transition, the workload _given \
         seam) must be an explicitly-reviewed entry, counted with multiplicity"
    );
    assert_eq!(
        sorted(test_support_method_names),
        sorted(TEST_SUPPORT_METHODS.iter().map(|s| s.to_string()).collect()),
        "the checkout_runtime module's `test-support`-only method set changed — the deterministic \
         substituted-execution seam must be an explicitly-reviewed entry, counted with \
         multiplicity, and it must NOT appear in the five-entry production accessor ALLOWLIST"
    );
}

/// **CT-007 slice 5b.3-6e.2: the activated cycle has exactly one checkout orchestration path and
/// the capsule's workload `OciConfig` is never detached.** The typed `run_cycle` selector is the
/// single production caller; the continuation and capsule transition remain single-sourced.
#[test]
fn the_checkout_runtime_capsule_has_exactly_one_activated_cycle_caller() {
    let prod = production_source();
    // The activated typed-cycle selector calls the outer orchestrator exactly once for a
    // checkout-bearing spec.
    assert_eq!(
        prod.matches(".launch_checkout_orchestrated_with(").count(),
        1,
        "the outer orchestrator has exactly ONE caller — the activated `run_cycle` selector"
    );
    assert_eq!(
        prod.matches("fn launch_checkout_orchestrated_with(")
            .count(),
        1,
        "the activated outer orchestrator is defined exactly once"
    );
    assert_eq!(
        prod.matches("fn launch_checkout_orchestrated_with_given")
            .count(),
        1,
        "its shared injectable body is defined exactly once"
    );
    // The fused V2 Hop B entry lives in the submodule (ONE definition); the continuation is its ONE
    // production caller — reachable only through the typed-cycle orchestrator.
    assert_eq!(
        CHECKOUT_RUNTIME_SOURCE
            .matches("run_checkout_preparation_v2(")
            .count(),
        1,
        "run_checkout_preparation_v2 is defined exactly once in the submodule"
    );
    assert_eq!(
        prod.matches("run_checkout_preparation_v2(").count(),
        1,
        "the continuation is the ONLY production caller of the fused Hop B entry"
    );
    // The capsule constructor is called ONLY by the activated orchestrator.
    assert_eq!(
        prod.matches("AcquiredCheckoutRuntime::acquire(").count(),
        1,
        "the capsule is constructed only by the activated orchestrator"
    );
    assert_eq!(
        CHECKOUT_RUNTIME_SOURCE
            .matches("AcquiredCheckoutRuntime::acquire(")
            .count(),
        0,
        "the submodule only DEFINES `acquire`, never calls the qualified form"
    );
    // The closed workload transition is invoked ONLY by the activated continuation.
    assert_eq!(
        prod.matches(".run_retained_workload(").count(),
        1,
        "the closed workload transition is invoked only by the activated continuation"
    );
    // The old free-standing `into_prepared`/`bind_workload` seams are gone: the ONLY prepared
    // transition is the fused Hop B entry (production) plus the audited `#[cfg(test)]`
    // `into_prepared_for_tests` (exactly one, test-only).
    assert_eq!(
        prod.matches("fn into_prepared").count(),
        0,
        "no production free-standing prepared transition — Hop B and the transition are fused"
    );
    assert_eq!(
        CHECKOUT_RUNTIME_SOURCE
            .matches("fn into_prepared_for_tests(")
            .count(),
        1,
        "the ONLY prepared transition outside the fused Hop B entry is the test-only one"
    );
    // The fused Hop B entry consumes the capsule by value and returns the prepared capsule.
    let v2_entry = source_of_in(
        CHECKOUT_RUNTIME_SOURCE,
        "pub(crate) fn run_checkout_preparation_v2(",
    );
    assert!(
        v2_entry.contains("mut runtime: AcquiredCheckoutRuntime")
            && v2_entry.contains(
                "-> Result<PreparedCheckoutRuntime, (AcquiredCheckoutRuntime, CheckoutPreparationError)>"
            ),
        "the fused Hop B entry consumes the capsule by value and returns the prepared capsule"
    );
    // 5b.3-6a/6b/6c: launch_with's OWN control flow never names either capsule type. In 6b
    // launch_with became a plain delegating wrapper and the compute body moved to
    // launch_compute_with; the span source_of captures for `fn launch_with<F>(` runs to this
    // impl's close (launch_with + launch_compute_with + dispose_run_failure) — the checkout seam
    // lives in SEPARATE impls, so it is deliberately outside this span.
    let launch_with = source_of("fn launch_with<F>(");
    assert!(
        !launch_with.contains("AcquiredCheckoutRuntime")
            && !launch_with.contains("PreparedCheckoutRuntime"),
        "the compute launch path (launch_with wrapper + launch_compute_with) names no capsule type"
    );
    assert!(
        launch_with.contains("self.launch_compute_with(spec, hooks, run)"),
        "launch_with is a plain delegating wrapper — it performs NO shape dispatch on spec.workspace"
    );
    // The continuation is DEFINED once and called ONLY by the activated orchestrator. It
    // consumes the capsule BY VALUE.
    assert_eq!(
        prod.matches("fn launch_checkout_continuation(").count(),
        1,
        "the checkout continuation is defined exactly once in production"
    );
    assert_eq!(
        prod.matches(".launch_checkout_continuation(").count(),
        1,
        "the continuation's ONLY caller is the activated orchestrator"
    );
    let seam = source_of("fn launch_checkout_continuation(");
    assert!(
        seam.contains("runtime: checkout_runtime::AcquiredCheckoutRuntime"),
        "the continuation consumes the capsule by value"
    );
    // Blocker 2/5: `acquire` retains the workload OciConfig INSIDE the capsule — its signature
    // returns the bare capsule, never `(OciConfig, ..)` detached.
    let acquire_sig = {
        let s = CHECKOUT_RUNTIME_SOURCE
            .find("pub(crate) fn acquire(")
            .expect("acquire exists in the submodule");
        let rest = &CHECKOUT_RUNTIME_SOURCE[s..];
        &rest[..rest.find(" {\n").expect("acquire signature ends")]
    };
    assert!(
        // CT-007 5b.3-6c (Sol's r2 finding 1): acquire now returns a TYPED `AcquisitionFailure`
        // (clean-refusal vs reconciliation-required), never a bare `String` — still the bare capsule
        // on success, never a detached `OciConfig`.
        acquire_sig.contains("-> Result<AcquiredCheckoutRuntime, AcquisitionFailure>")
            && !acquire_sig.contains("OciConfig"),
        "acquire must return the capsule alone — never the workload OciConfig detached"
    );

    // ── CT-007 slice 5b.3-6e.2: the activated compute-V2 entry + checkout config ──
    assert_eq!(
        prod.matches("fn launch_compute_orchestrated_with").count(),
        1,
        "the activated compute-V2 orchestrated entry is defined exactly once"
    );
    // The activated typed-cycle selector calls the compute-V2 orchestrated entry exactly once.
    assert_eq!(
        prod.matches(".launch_compute_orchestrated_with(").count(),
        1,
        "the compute-V2 orchestrated entry has exactly ONE caller — the activated `run_cycle` selector"
    );
    // The shared post-reservation body is extracted ONCE and used by BOTH compute entries; the
    // preflight is extracted once and used by both. The legacy `launch_compute_with` remains the
    // compatibility compute entry (called by the plain `launch_with` wrapper and streaming).
    assert_eq!(
        prod.matches("fn launch_compute_common_body").count(),
        1,
        "the shared post-reservation compute body is defined exactly once"
    );
    assert_eq!(
        prod.matches(".launch_compute_common_body(").count(),
        2,
        "both compute entries (legacy compatibility + activated orchestrated) run the ONE shared common body"
    );
    assert_eq!(
        prod.matches("fn compute_launch_preflight").count(),
        1,
        "the shared compute preflight is defined exactly once"
    );
    assert_eq!(
        prod.matches(".compute_launch_preflight(").count(),
        2,
        "both compute entries run the ONE shared preflight"
    );
    // The checkout repository-root config: every production GvisorBackend constructor leaves it
    // `disabled()`. This is a gvisor.rs-LOCAL invariant; the CROSS-CRATE composition-root zero for
    // the `with_checkout_config` selector (which a controlplane root could call) is enforced by the
    // recursive both-crates dormancy scan `the_v2_phase_credential_surface_has_exactly_its_known_occurrences`.
    assert_eq!(
        prod.matches("checkout: GvisorCheckoutConfig::disabled()")
            .count(),
        3,
        "every production GvisorBackend constructor leaves checkout disabled()"
    );
}

/// **CT-007 slice 5b.3-6e.1 (Sol's blocker 1): the invalid state is UNCONSTRUCTABLE.** `enabled()`
/// is structurally the ONLY path to an enabled config — the wrapped `CheckoutConfigState` is
/// private, so no external construction of `Enabled { repo_root: <unvalidated> }` is possible, and
/// `with_checkout_config` therefore can only ever receive an already-validated value. This test
/// pins the two facts a reviewer can check: `disabled()` carries no root, and the ONLY
/// `CheckoutConfigState::Enabled` construction in production source is inside `fn enabled(` (the
/// validating constructor). A future `CheckoutConfigState::Enabled { .. }` built anywhere else —
/// bypassing validation — trips this pin.
#[test]
fn an_enabled_checkout_config_can_only_arise_from_the_validating_constructor() {
    assert_eq!(GvisorCheckoutConfig::disabled().repo_root(), None);

    // Every enabled-state CONSTRUCTION in production source (test module stripped) must sit inside
    // the validating `fn enabled(`. The construction is uniquely spelled
    // `GvisorCheckoutConfig(CheckoutConfigState::Enabled {` (the wrapper prefix distinguishes it
    // from `repo_root()`'s bare `CheckoutConfigState::Enabled { .. } =>` match pattern). There is
    // exactly ONE, and it is that site.
    let prod = production_source();
    let construction = "GvisorCheckoutConfig(CheckoutConfigState::Enabled {";
    assert_eq!(
        prod.matches(construction).count(),
        1,
        "there must be exactly one enabled-config construction site in production source"
    );
    let enabled_fn = source_of("pub fn enabled(");
    assert_eq!(
        enabled_fn.matches(construction).count(),
        1,
        "the sole enabled-config construction lives inside the boot-validating `enabled()` — no \
         other code path can build an enabled config with an unvalidated path"
    );
}

// CT-007 slice 5b.3-6c: the r3/r4 COUNTING pins were all REMOVED — Sol compiled evasions of each.
// The terminal fences are LANGUAGE-ENFORCED (mirroring 6a's RAII + module-privacy discipline):
//   - FINDING 1 (resource safety): an RAII `NotStartedCapsuleGuard` owns the capsule before Hop B;
//     its `Drop` performs the SAFE NotStarted cleanup (delete + release_unused), so EVERY early
//     return / `?` / unwind before Hop B disposes safely — the manager stays healthy, the slot
//     reusable — with no syntactic pin. The success path `disarm()`s it into `hop_b`. See
//     `launch_checkout_continuation_given` + the always-run
//     `not_started_capsule_guard_disposes_safely_on_any_early_exit` proof.
//   - FINDING 2 (credential substitution): `WorkloadRotatedSpec` lives in the sealed `workload_spec`
//     module (private field, no `as_job_spec`, no `Clone`/`From`), and its inner `JobSpec` is
//     consumed ONLY by its own `acquire_permit_and_run` — so no outer code can obtain a `&JobSpec`
//     to clone/substitute (`error[E0599]: no method named as_job_spec`). See
//     `the_workload_spec_module_shape_is_pinned`.

/// **Sol's r5/r6 finding 2 (CLOSED-WORLD module audit): the workload-spec wrapper never leaks its
/// inner `JobSpec`.** Mirrors the 6a `checkout_runtime` module discipline. `workload_spec.rs`'s
/// ENTIRE surface is EXACTLY the `WorkloadRotatedSpec` struct (private field, no `Clone`/`Copy`), the
/// `BoundWorkloadRefusal` enum, and ONE inherent impl whose PRODUCTION methods are `{from_carrier,
/// acquire_permit_and_run}`, whose private helper is `{acquire_permit_and_prep}`, and whose
/// `#[cfg(test)]`-only method is `{acquire_permit_and_run_given}` — every set pinned EXACTLY. NO
/// method (production, private, OR test) may return a type that MENTIONS `JobSpec` at ANY nesting
/// (`&JobSpec`, `Result<&JobSpec,_>`, a tuple containing one, `Option<&JobSpec>`, `impl
/// Deref<Target=JobSpec>`, …) — the whole return-type AST is walked for the `JobSpec` ident. And NO
/// trait impl (a `Clone`/`From`/`Deref` could hand out the inner spec). Any leak-adding item/accessor
/// fails this audit BY NAME, fail-closed.
#[test]
fn the_workload_spec_module_shape_is_pinned() {
    const SOURCE: &str = include_str!("workload_spec.rs");
    let file = syn::parse_file(SOURCE).expect("workload_spec.rs parses as a File");

    fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            let mut hit = false;
            if a.path().is_ident("cfg") {
                let _ = a.parse_nested_meta(|meta| {
                    if meta.path.is_ident("test") {
                        hit = true;
                    }
                    Ok(())
                });
            }
            hit
        })
    }
    // Gated `#[cfg(any(test, feature = "test-support"))]` — the test-support permit-fence seam,
    // distinct from the `#[cfg(test)]`-only injectable-execute seam. Its own pinned inventory.
    fn is_cfg_test_support(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            a.path().is_ident("cfg")
                && matches!(&a.meta, syn::Meta::List(list) if list.tokens.to_string().contains("test-support"))
        })
    }
    // Walk the ENTIRE return-type AST for the `JobSpec` ident — nested/opaque returns included.
    fn type_mentions_job_spec(ty: &syn::Type) -> bool {
        use syn::visit::Visit;
        struct Scan {
            found: bool,
        }
        impl<'ast> Visit<'ast> for Scan {
            fn visit_ident(&mut self, id: &'ast syn::Ident) {
                if id == "JobSpec" {
                    self.found = true;
                }
            }
        }
        let mut scan = Scan { found: false };
        scan.visit_type(ty);
        scan.found
    }

    let mut struct_seen = false;
    let mut enum_seen = false;
    let mut production_methods: Vec<String> = Vec::new();
    let mut private_methods: Vec<String> = Vec::new();
    let mut test_methods: Vec<String> = Vec::new();
    let mut test_support_methods: Vec<String> = Vec::new();
    let mut violations: Vec<String> = Vec::new();
    for item in &file.items {
        match item {
            syn::Item::Use(_) => {}
            syn::Item::Struct(s) if s.ident == "WorkloadRotatedSpec" => {
                struct_seen = true;
                for f in &s.fields {
                    if !matches!(f.vis, syn::Visibility::Inherited) {
                        violations.push("WorkloadRotatedSpec field is not private".to_string());
                    }
                }
                for attr in &s.attrs {
                    if attr.path().is_ident("derive") {
                        let _ = attr.parse_nested_meta(|m| {
                            if m.path.is_ident("Clone") || m.path.is_ident("Copy") {
                                violations.push(
                                    "WorkloadRotatedSpec derives Clone/Copy — could duplicate the \
                                     inner spec"
                                        .to_string(),
                                );
                            }
                            Ok(())
                        });
                    }
                }
            }
            syn::Item::Enum(e) if e.ident == "BoundWorkloadRefusal" => enum_seen = true,
            syn::Item::Impl(im) if im.trait_.is_some() => violations.push(
                "a trait impl in the workload_spec module could hand out the inner spec (e.g. \
                 Clone/From/Deref)"
                    .to_string(),
            ),
            syn::Item::Impl(im) => {
                for it in &im.items {
                    match it {
                        syn::ImplItem::Fn(m) => {
                            let name = m.sig.ident.to_string();
                            // NO method may return a type MENTIONING `JobSpec` at any nesting.
                            if let syn::ReturnType::Type(_, ty) = &m.sig.output {
                                if type_mentions_job_spec(ty) {
                                    violations.push(format!(
                                        "method `{name}` returns a type mentioning `JobSpec` — the \
                                         inner spec must never escape to be cloned/substituted"
                                    ));
                                }
                            }
                            if is_cfg_test(&m.attrs) {
                                test_methods.push(name);
                            } else if is_cfg_test_support(&m.attrs) {
                                test_support_methods.push(name);
                            } else if matches!(m.vis, syn::Visibility::Inherited) {
                                private_methods.push(name);
                            } else {
                                production_methods.push(name);
                            }
                        }
                        _ => violations.push(
                            "non-fn associated item in the WorkloadRotatedSpec impl".to_string(),
                        ),
                    }
                }
            }
            other => violations.push(format!(
                "unexpected top-level item in workload_spec (closed-world): {:?}",
                std::mem::discriminant(other)
            )),
        }
    }
    assert!(
        violations.is_empty(),
        "workload_spec module shape violated: {violations:#?}"
    );
    assert!(
        struct_seen && enum_seen,
        "the two sanctioned types must be present"
    );
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    assert_eq!(
        sorted(production_methods),
        vec![
            "acquire_permit_and_run".to_string(),
            "from_carrier".to_string()
        ],
        "the workload_spec PRODUCTION surface is EXACTLY the sealed constructor + the fixed-runner \
         method (which calls run_production_container_streaming itself — no caller `execute`)"
    );
    assert_eq!(
        sorted(private_methods),
        vec!["acquire_permit_and_prep".to_string()],
        "the ONLY private helper is the shared permit+prep step"
    );
    assert_eq!(
        sorted(test_methods),
        vec!["acquire_permit_and_run_given".to_string()],
        "the ONLY `#[cfg(test)]` method is the injectable execution seam — the sole place an \
         `execute` closure receiving `&JobSpec` exists, absent from every ordinary build"
    );
    assert_eq!(
        sorted(test_support_methods),
        vec!["acquire_launch_permit_for_test_support".to_string()],
        "the ONLY `#[cfg(any(test, feature = \"test-support\"))]` method is the sealed permit-fence \
         acquisition the deterministic runsc-driver seam drives (it acquires against `&self.spec` \
         and returns only a LaunchPermit — the inner spec never escapes)"
    );
}

/// **The one-shot fetch provider is invoked after the advertisement retires and the lease
/// checkpoint renews, and before ANYTHING for the fetch is built or spawned.** Ordering is the
/// whole security property here: minting the fetch credential earlier would let it be issued
/// against a generation this worker may no longer own.
#[test]
fn the_fetch_phase_authorization_is_obtained_between_the_checkpoint_and_the_fetch_spawn() {
    let transport = source_of("fn fetch_checkout_pack_within_parent_attempt_inner(");
    let advertise_hop = transport
        .find("false, // this is the FIRST hop")
        .expect("the advertise hop runs");
    let checkpoint = transport
        .find("if let Some(checkpoint) = lease_checkpoint {")
        .expect("the lease checkpoint runs");
    let provider = transport
        .find("let (fetch_run_token, fetch_permit) = match fetch_source.as_mut()")
        .expect("the fetch authorization is obtained");
    let fetch_spec = transport
        .find("let fetch_spec = GitWireSpec::for_repo(")
        .expect("the fetch spec is built");
    let fetch_hop = transport
        .find("true, // the advertisement hop above already completed.")
        .expect("the fetch hop runs");
    assert!(
        advertise_hop < checkpoint
            && checkpoint < provider
            && provider < fetch_spec
            && fetch_spec < fetch_hop,
        "ordering must be advertise -> renew -> mint fetch credential -> build -> spawn"
    );
}

#[test]
fn production_streaming_entries_install_one_job_wide_total_log_cap() {
    let streaming = source_of("fn launch_streaming(");
    assert!(
        streaming.find("cap_total_job_output(output)").unwrap()
            < streaming.find("self.launch_with(").unwrap(),
        "ordinary streaming launch must install the cap before any production run path"
    );

    let cycle = source_of("fn run_cycle(");
    assert!(
        cycle.find("cap_total_job_output(output)").unwrap()
            < cycle
                .find("match crate::derive_checkout_authorization_scope")
                .unwrap(),
        "the cap must wrap the sink once before checkout routing so every phase shares it"
    );
}
