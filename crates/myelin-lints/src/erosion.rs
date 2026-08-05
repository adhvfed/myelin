use proc_macro2::Span;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, ForeignItem, ImplItem, Item, TraitItem};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ErosionBudget {
    pub soft_limit: usize,
    pub hard_limit: usize,
    #[serde(default)]
    pub over_limit: Vec<OverLimit>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct OverLimit {
    pub path: String,
    pub production_lines: usize,
}

pub fn parse_budget(source: &str) -> Result<ErosionBudget, String> {
    let budget: ErosionBudget = toml::from_str(source).map_err(|error| error.to_string())?;
    if budget.soft_limit == 0 || budget.hard_limit < budget.soft_limit {
        return Err("erosion limits must satisfy 0 < soft_limit <= hard_limit".into());
    }
    let mut paths = BTreeSet::new();
    for entry in &budget.over_limit {
        if entry.path.is_empty() || !paths.insert(entry.path.as_str()) {
            return Err(format!(
                "duplicate or empty erosion allowlist path `{}`",
                entry.path
            ));
        }
        if entry.production_lines <= budget.soft_limit {
            return Err(format!(
                "erosion allowlist `{}` is not above soft_limit {}",
                entry.path, budget.soft_limit
            ));
        }
    }
    Ok(budget)
}

pub fn test_line_ranges(source: &str) -> Result<Vec<(usize, usize)>, String> {
    let syntax = syn::parse_file(source).map_err(|error| error.to_string())?;
    let mut visitor = TestRangeVisitor::default();
    visitor.visit_file(&syntax);
    Ok(visitor.ranges)
}

pub fn production_lines(source: &str) -> Result<usize, String> {
    let mut excluded = BTreeSet::new();
    for (start, end) in test_line_ranges(source)? {
        for line in start..=end {
            excluded.insert(line);
        }
    }
    Ok(source.lines().count().saturating_sub(excluded.len()))
}

pub fn scan_workspace(root: &Path, budget: &ErosionBudget) -> Vec<String> {
    let mut files = Vec::new();
    collect_rust_files(&root.join("crates"), &mut files);
    files.sort();
    let allowances = budget
        .over_limit
        .iter()
        .map(|entry| (entry.path.as_str(), entry.production_lines))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut errors = Vec::new();
    for path in files {
        if !path.components().any(|part| part.as_os_str() == "src") {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                errors.push(format!("L2: cannot read `{relative}`: {error}"));
                continue;
            }
        };
        let count = match production_lines(&source) {
            Ok(count) => count,
            Err(error) => {
                errors.push(format!("L2: cannot parse `{relative}`: {error}"));
                continue;
            }
        };
        let allowance = allowances.get(relative.as_ref()).copied();
        if count > budget.soft_limit {
            seen.insert(relative.to_string());
            match allowance {
                None => errors.push(format!(
                    "L2: `{relative}` has {count} production lines (soft {}, hard {}) but is not in the shrink-only allowlist",
                    budget.soft_limit, budget.hard_limit
                )),
                Some(expected) if count > expected => errors.push(format!(
                    "L2: `{relative}` grew from its {expected}-line allowance to {count} production lines"
                )),
                Some(expected) if count < expected => errors.push(format!(
                    "L2: `{relative}` shrank to {count} production lines; lower its stale {expected}-line allowance"
                )),
                Some(_) => {}
            }
        } else if allowance.is_some() {
            errors.push(format!(
                "L2: `{relative}` is now within the {}-line soft ceiling; remove its allowance",
                budget.soft_limit
            ));
            seen.insert(relative.to_string());
        }
        if count > budget.hard_limit && allowance.is_none() {
            errors.push(format!(
                "L2: `{relative}` breaches the {}-line hard ceiling",
                budget.hard_limit
            ));
        }
    }
    for entry in &budget.over_limit {
        if !seen.contains(&entry.path) {
            errors.push(format!(
                "L2: allowlisted module `{}` does not exist under crates/*/src",
                entry.path
            ));
        }
    }
    errors
}

fn collect_rust_files(path: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

#[derive(Default)]
struct TestRangeVisitor {
    ranges: Vec<(usize, usize)>,
}

impl TestRangeVisitor {
    fn exclude(&mut self, span: Span) {
        let start = span.start().line;
        let end = span.end().line.max(start);
        self.ranges.push((start, end));
    }
}

impl<'ast> Visit<'ast> for TestRangeVisitor {
    fn visit_item(&mut self, node: &'ast Item) {
        if item_attributes(node).is_some_and(cfg_test) {
            self.exclude(node.span());
        } else {
            visit::visit_item(self, node);
        }
    }

    fn visit_impl_item(&mut self, node: &'ast ImplItem) {
        if impl_item_attributes(node).is_some_and(cfg_test) {
            self.exclude(node.span());
        } else {
            visit::visit_impl_item(self, node);
        }
    }

    fn visit_trait_item(&mut self, node: &'ast TraitItem) {
        if trait_item_attributes(node).is_some_and(cfg_test) {
            self.exclude(node.span());
        } else {
            visit::visit_trait_item(self, node);
        }
    }

    fn visit_foreign_item(&mut self, node: &'ast ForeignItem) {
        if foreign_item_attributes(node).is_some_and(cfg_test) {
            self.exclude(node.span());
        } else {
            visit::visit_foreign_item(self, node);
        }
    }
}

fn item_attributes(item: &Item) -> Option<&[Attribute]> {
    Some(match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        Item::Verbatim(_) => return None,
        _ => return None,
    })
}

fn impl_item_attributes(item: &ImplItem) -> Option<&[Attribute]> {
    Some(match item {
        ImplItem::Const(item) => &item.attrs,
        ImplItem::Fn(item) => &item.attrs,
        ImplItem::Type(item) => &item.attrs,
        ImplItem::Macro(item) => &item.attrs,
        ImplItem::Verbatim(_) => return None,
        _ => return None,
    })
}

fn trait_item_attributes(item: &TraitItem) -> Option<&[Attribute]> {
    Some(match item {
        TraitItem::Const(item) => &item.attrs,
        TraitItem::Fn(item) => &item.attrs,
        TraitItem::Type(item) => &item.attrs,
        TraitItem::Macro(item) => &item.attrs,
        TraitItem::Verbatim(_) => return None,
        _ => return None,
    })
}

fn foreign_item_attributes(item: &ForeignItem) -> Option<&[Attribute]> {
    Some(match item {
        ForeignItem::Fn(item) => &item.attrs,
        ForeignItem::Static(item) => &item.attrs,
        ForeignItem::Type(item) => &item.attrs,
        ForeignItem::Macro(item) => &item.attrs,
        ForeignItem::Verbatim(_) => return None,
        _ => return None,
    })
}

fn cfg_test(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && attribute
                .meta
                .require_list()
                .is_ok_and(|list| list.tokens.to_string() == "test")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("myelin-erosion-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("crates/demo/src")).unwrap();
        root
    }

    #[test]
    fn exact_cfg_test_items_do_not_consume_the_production_budget() {
        let source = "pub fn live() {}\n#[cfg(test)]\nstruct Fixture;\n#[cfg(test)]\nmod tests {\n fn helper() {}\n}\n";
        assert_eq!(production_lines(source).unwrap(), 1);
    }

    #[test]
    fn exact_cfg_test_associated_items_do_not_consume_the_production_budget() {
        let source = "struct Live;\nimpl Live {\n#[cfg(test)]\nfn fixture() {}\n}\n";
        assert_eq!(production_lines(source).unwrap(), 3);
    }

    #[test]
    fn test_support_code_remains_in_the_production_feature_budget() {
        let source = "#[cfg(any(test, feature = \"test-support\"))]\npub fn fixture() {}\n";
        assert_eq!(production_lines(source).unwrap(), 2);
    }

    #[test]
    fn exact_allowance_is_green_but_growth_and_stale_headroom_are_red() {
        let root = fixture_root("ratchet");
        let path = root.join("crates/demo/src/lib.rs");
        std::fs::write(&path, "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
        let budget = ErosionBudget {
            soft_limit: 2,
            hard_limit: 4,
            over_limit: vec![OverLimit {
                path: "crates/demo/src/lib.rs".into(),
                production_lines: 3,
            }],
        };
        assert!(scan_workspace(&root, &budget).is_empty());

        std::fs::write(&path, "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n").unwrap();
        assert!(scan_workspace(&root, &budget)
            .iter()
            .any(|error| error.contains("grew")));

        std::fs::write(&path, "fn a() {}\nfn b() {}\n").unwrap();
        assert!(scan_workspace(&root, &budget)
            .iter()
            .any(|error| error.contains("remove its allowance")));
        std::fs::remove_dir_all(root).ok();
    }
}
