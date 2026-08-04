use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_content::InlineNode;
use myelin_identity::Principal;

pub const DEFAULT_HOT_SUBJECT_WRITE_CAP: u32 = 64;

pub fn extract_mentions(nodes: &[InlineNode]) -> Vec<Principal> {
    let mut out: Vec<Principal> = Vec::new();
    for node in nodes {
        if let InlineNode::Mention(principal) = node {
            let already = out.iter().any(|p| p.principal_id == principal.principal_id);
            if !already {
                out.push(principal.clone());
            }
        }
    }
    out
}

#[derive(Clone)]
pub struct HotSubjectCap {
    cap: u32,
    admitted: Arc<Mutex<HashMap<String, RootState>>>,
}

#[derive(Default)]
struct RootState {
    admitted: std::collections::HashSet<String>,
    overflowed: std::collections::HashSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapVerdict {
    Admit,
    Overflow,
}

impl HotSubjectCap {
    pub fn new() -> HotSubjectCap {
        HotSubjectCap::with_cap(DEFAULT_HOT_SUBJECT_WRITE_CAP)
    }

    pub fn with_cap(cap: u32) -> HotSubjectCap {
        HotSubjectCap {
            cap,
            admitted: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn cap(&self) -> u32 {
        self.cap
    }

    pub fn admit(&self, recipient: &str, subject_root: &str) -> CapVerdict {
        let mut g = self.admitted.lock().unwrap_or_else(|e| e.into_inner());
        let state = g.entry(subject_root.to_string()).or_default();
        if state.admitted.contains(recipient) {
            return CapVerdict::Admit;
        }
        if (state.admitted.len() as u32) < self.cap {
            state.admitted.insert(recipient.to_string());
            CapVerdict::Admit
        } else {
            state.overflowed.insert(recipient.to_string());
            CapVerdict::Overflow
        }
    }

    pub fn admitted_count(&self, subject_root: &str) -> u32 {
        self.admitted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_root)
            .map(|s| s.admitted.len() as u32)
            .unwrap_or(0)
    }

    pub fn overflow_count(&self, subject_root: &str) -> u32 {
        self.admitted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_root)
            .map(|s| s.overflowed.len() as u32)
            .unwrap_or(0)
    }
}

impl Default for HotSubjectCap {
    fn default() -> HotSubjectCap {
        HotSubjectCap::new()
    }
}

#[cfg(test)]
mod tests;
