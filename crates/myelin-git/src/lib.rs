#![forbid(unsafe_code)]

pub mod agent_author;
pub mod anchor;
pub mod backup;
pub mod blob_coordinate;
pub mod body;
pub mod check_status;
pub mod check_status_store;
pub mod code_projection;
pub mod code_tools;
pub mod commit;
pub mod coordinate {
    pub use myelin_refs::git_coordinate::*;
}
pub mod core;
pub mod cross_cell;
pub mod durable;
pub mod events;
pub mod fork_gate;
pub mod front_door;
pub mod git_resolve;
pub mod gix_backend;
pub mod lifecycle;
pub mod list_filter;
pub mod live_check;
pub mod merge_gate;
pub mod merge_queue;
pub mod notif_rules;
pub mod object_format;
pub mod object_packs;
pub mod pack_tier;
pub mod patch_id_chain;
mod pg_pr_event;
pub mod pg_pr_store;
pub mod pr_list_pagination;
pub mod pr_store;
pub mod pr_threads;
pub mod project;
pub mod rebac_fragment;
pub mod receive_pack;
pub mod reconcile;
pub mod refs_pagination;
pub mod replay;
pub mod schema;
pub mod scip;
pub mod search_projection;
pub mod shed_clone;
pub mod speculative_queue;
pub mod subs;
pub mod surge;
pub mod tree_pagination;
pub mod typed_edges;

pub mod web;

pub mod api;
