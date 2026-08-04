use super::*;

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

const CHECKOUT_RUNTIME_SOURCE: &str = include_str!("checkout_runtime.rs");

fn production_of(source: &'static str) -> &'static str {
    let end = source
        .find("\n#[cfg(test)]\nmod tests {")
        .or_else(|| source.find(TEST_MODULE_BANNER));
    match end {
        Some(end) => &source[..end],
        None => source,
    }
}

const TEST_MODULE_BANNER: &str = "\n// ══════ the test and test-support modules ══════";

pub(super) fn production_source() -> String {
    PRODUCTION_SOURCES
        .iter()
        .map(|source| production_of(source))
        .collect::<Vec<_>>()
        .join("\n")
}

fn source_in(source: &'static str, function_signature: &str) -> Option<&'static str> {
    let start = source.find(function_signature)?;
    let rest = &source[start..];
    let end = rest.find("\n}\n")?;
    Some(&rest[..end])
}

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
    assert_eq!(
        AUTHORIZATION_SOURCE.matches("self.permit").count(),
        2,
        "the permit field is TOUCHED in exactly two places: the two consuming into_*_permit \
         methods (transport, and the 5b.3-6a full-scope preparation - the commit-only preparation \
         permit was removed in r2). A new permit-exposing method would raise this count."
    );
    assert_eq!(
        AUTHORIZATION_SOURCE.matches("Ok(self.permit)").count(),
        2,
        "the permit escapes only through the two consuming into_*_permit methods"
    );
    let impl_block = {
        let start = AUTHORIZATION_SOURCE
            .find("\nimpl PhaseAuthorization {")
            .expect("the impl block exists");
        let rest = &AUTHORIZATION_SOURCE[start + 1..];
        let end = rest.find("\n}\n").expect("the impl block closes");
        &rest[..end]
    };
    fn method_name(line: &str) -> Option<&str> {
        let mut rest = line.trim_start();
        if let Some(after_pub) = rest.strip_prefix("pub") {
            if let Some(inner) = after_pub.strip_prefix('(') {
                let close = inner.find(')')?;
                rest = inner[close + 1..].trim_start();
            } else if after_pub.starts_with(char::is_whitespace) {
                rest = after_pub.trim_start();
            }
        }
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
        "the PhaseAuthorization method surface changed - any new method (under ANY visibility or \
         modifier) that could move or expose `self.permit` (or destructure self) must be \
         reviewed and pinned here"
    );
    assert_eq!(
        impl_block.matches("Self {").count(),
        0,
        "no `Self {{ .. }}` destructuring pattern inside impl PhaseAuthorization may bind the \
         private permit"
    );
    assert_eq!(
        impl_block.matches("PhaseAuthorization {").count(),
        1,
        "the only `PhaseAuthorization {{` inside the impl block is its own header - never a \
         destructuring pattern that could pull the permit out"
    );
    assert_eq!(
        AUTHORIZATION_SOURCE.matches("PhaseAuthorization {").count(),
        4,
        "exactly: the struct decl, the Debug impl header, the inherent impl header, and the one \
         construction site - no destructuring pattern reaches the private permit"
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
    const ALLOWLIST: [&str; 5] = [
        "AcquiredCheckoutRuntime::acquire",
        "AcquiredCheckoutRuntime::dispose_checkout_runtime",
        "PreparedCheckoutRuntime::dispose_checkout_runtime",
        "PreparedCheckoutRuntime::run_retained_workload",
        "run_checkout_preparation_v2",
    ];
    const TEST_METHODS: [&str; 3] = [
        "drive_session_for_tests",
        "into_prepared_for_tests",
        "run_retained_workload_given",
    ];
    const TEST_SUPPORT_METHODS: [&str; 2] = [
        "substituted_hop_b_for_test_support",
        "substituted_workload_for_test_support",
    ];
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
    fn is_cfg_test_support(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            a.path().is_ident("cfg")
                && matches!(&a.meta, syn::Meta::List(list) if list.tokens.to_string().contains("test-support"))
        })
    }
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
    let mut accessor_surface: Vec<String> = Vec::new();
    let mut test_method_names: Vec<String> = Vec::new();
    let mut test_support_method_names: Vec<String> = Vec::new();
    let mut free_fn_names: Vec<String> = Vec::new();
    let mut struct_names: Vec<String> = Vec::new();

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
                        "macro `{path}!` in the capsule module mentions `{hit}` - `syn` cannot \
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

    for item in &file.items {
        match item {
            syn::Item::Use(_) => {}

            syn::Item::Struct(s) => {
                let name = s.ident.to_string();
                struct_names.push(name.clone());
                if !CAPSULES.contains(&name.as_str()) {
                    violations.push(format!(
                        "unexpected struct `{name}` - only the two capsule structs are permitted"
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

            syn::Item::Fn(f) => {
                let name = f.sig.ident.to_string();
                free_fn_names.push(name.clone());
                if !ALLOWED_FREE_FNS.contains(&name.as_str()) {
                    violations.push(format!(
                        "unexpected free fn `{name}` - the capsule module permits only \
                         {ALLOWED_FREE_FNS:?} at top level"
                    ));
                } else if !is_cfg_test(&f.attrs) && !is_private(&f.vis) {
                    accessor_surface.push(name);
                }
            }

            syn::Item::Impl(im) => {
                if im.trait_.is_some() {
                    violations.push(format!(
                        "trait impl on `{}` - forbidden (a `match self` could hand out the \
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
                                        test_method_names.push(m.sig.ident.to_string());
                                    } else if test_support_only {
                                        test_support_method_names.push(m.sig.ident.to_string());
                                    } else if !is_private(&m.vis) {
                                        accessor_surface.push(format!("{name}::{}", m.sig.ident));
                                    }
                                }
                                _ => violations.push(format!(
                                    "non-fn associated item in inherent impl of `{name}` - a \
                                     const/type could hand out an inner field"
                                )),
                            }
                        }
                    }
                    other => violations.push(format!(
                        "inherent impl on `{}` - only the two capsule types may be `impl`ed in \
                         this module",
                        other.unwrap_or("<non-path type>")
                    )),
                }
            }

            other => violations.push(format!(
                "forbidden top-level item in the capsule module: {} - its production surface is \
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

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }
    assert_eq!(
        sorted(struct_names),
        sorted(CAPSULES.iter().map(|s| s.to_string()).collect()),
        "the capsule struct-name list changed - EXACTLY the two capsule structs, no duplicate \
         (e.g. a second cfg-gated struct reusing a capsule name)"
    );
    assert_eq!(
        sorted(free_fn_names),
        sorted(ALLOWED_FREE_FNS.iter().map(|s| s.to_string()).collect()),
        "the top-level free-fn list changed - EXACTLY run_checkout_preparation_v2, counted with \
         multiplicity, so a second cfg-gated definition of that name fails"
    );
    assert_eq!(
        sorted(accessor_surface),
        sorted(ALLOWLIST.iter().map(|s| s.to_string()).collect()),
        "the checkout_runtime module's non-private accessor surface changed - every accessor must \
         be an explicitly-reviewed capsule entry, counted with multiplicity"
    );
    assert_eq!(
        sorted(test_method_names),
        sorted(TEST_METHODS.iter().map(|s| s.to_string()).collect()),
        "the checkout_runtime module's `#[cfg(test)]`-only method set changed - every test-only \
         capsule method (the session driver, the into_prepared transition, the workload _given \
         seam) must be an explicitly-reviewed entry, counted with multiplicity"
    );
    assert_eq!(
        sorted(test_support_method_names),
        sorted(TEST_SUPPORT_METHODS.iter().map(|s| s.to_string()).collect()),
        "the checkout_runtime module's `test-support`-only method set changed - the deterministic \
         substituted-execution seam must be an explicitly-reviewed entry, counted with \
         multiplicity, and it must NOT appear in the five-entry production accessor ALLOWLIST"
    );
}

#[test]
fn the_checkout_runtime_capsule_has_exactly_one_activated_cycle_caller() {
    let prod = production_source();
    assert_eq!(
        prod.matches(".launch_checkout_orchestrated_with(").count(),
        1,
        "the outer orchestrator has exactly ONE caller - the activated `run_cycle` selector"
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
    assert_eq!(
        prod.matches(".run_retained_workload(").count(),
        1,
        "the closed workload transition is invoked only by the activated continuation"
    );
    assert_eq!(
        prod.matches("fn into_prepared").count(),
        0,
        "no production free-standing prepared transition - Hop B and the transition are fused"
    );
    assert_eq!(
        CHECKOUT_RUNTIME_SOURCE
            .matches("fn into_prepared_for_tests(")
            .count(),
        1,
        "the ONLY prepared transition outside the fused Hop B entry is the test-only one"
    );
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
    let launch_with = source_of("fn launch_with<F>(");
    assert!(
        !launch_with.contains("AcquiredCheckoutRuntime")
            && !launch_with.contains("PreparedCheckoutRuntime"),
        "the compute launch path (launch_with wrapper + launch_compute_with) names no capsule type"
    );
    assert!(
        launch_with.contains("self.launch_compute_with(spec, hooks, run)"),
        "launch_with is a plain delegating wrapper - it performs NO shape dispatch on spec.workspace"
    );
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
    let acquire_sig = {
        let s = CHECKOUT_RUNTIME_SOURCE
            .find("pub(crate) fn acquire(")
            .expect("acquire exists in the submodule");
        let rest = &CHECKOUT_RUNTIME_SOURCE[s..];
        &rest[..rest.find(" {\n").expect("acquire signature ends")]
    };
    assert!(
        acquire_sig.contains("-> Result<AcquiredCheckoutRuntime, AcquisitionFailure>")
            && !acquire_sig.contains("OciConfig"),
        "acquire must return the capsule alone - never the workload OciConfig detached"
    );

    assert_eq!(
        prod.matches("fn launch_compute_orchestrated_with").count(),
        1,
        "the activated compute-V2 orchestrated entry is defined exactly once"
    );
    assert_eq!(
        prod.matches(".launch_compute_orchestrated_with(").count(),
        1,
        "the compute-V2 orchestrated entry has exactly ONE caller - the activated `run_cycle` selector"
    );
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
    assert_eq!(
        prod.matches("checkout: GvisorCheckoutConfig::disabled()")
            .count(),
        3,
        "every production GvisorBackend constructor leaves checkout disabled()"
    );
}

#[test]
fn an_enabled_checkout_config_can_only_arise_from_the_validating_constructor() {
    assert_eq!(GvisorCheckoutConfig::disabled().repo_root(), None);

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
        "the sole enabled-config construction lives inside the boot-validating `enabled()` - no \
         other code path can build an enabled config with an unvalidated path"
    );
}

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
    fn is_cfg_test_support(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            a.path().is_ident("cfg")
                && matches!(&a.meta, syn::Meta::List(list) if list.tokens.to_string().contains("test-support"))
        })
    }
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
                                    "WorkloadRotatedSpec derives Clone/Copy - could duplicate the \
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
                            if let syn::ReturnType::Type(_, ty) = &m.sig.output {
                                if type_mentions_job_spec(ty) {
                                    violations.push(format!(
                                        "method `{name}` returns a type mentioning `JobSpec` - the \
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
         method (which calls run_production_container_streaming itself - no caller `execute`)"
    );
    assert_eq!(
        sorted(private_methods),
        vec!["acquire_permit_and_prep".to_string()],
        "the ONLY private helper is the shared permit+prep step"
    );
    assert_eq!(
        sorted(test_methods),
        vec!["acquire_permit_and_run_given".to_string()],
        "the ONLY `#[cfg(test)]` method is the injectable execution seam - the sole place an \
         `execute` closure receiving `&JobSpec` exists, absent from every ordinary build"
    );
    assert_eq!(
        sorted(test_support_methods),
        vec!["acquire_launch_permit_for_test_support".to_string()],
        "the ONLY `#[cfg(any(test, feature = \"test-support\"))]` method is the sealed permit-fence \
         acquisition the deterministic runsc-driver seam drives (it acquires against `&self.spec` \
         and returns only a LaunchPermit - the inner spec never escapes)"
    );
}

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
