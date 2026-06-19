# Phase 7-B — Coverage Matrix (every roadmap milestone → its prompt id(s))

> Phase: `07-prompts`. The **coverage verification** for the Myelin prompt ledger. The binding guarantee of
> the ledger (`00-ledger-overview.md` §5): **every roadmap milestone across all 16 systems maps to at least one
> prompt** — nothing in Phase 6 is silently dropped on the way to Phase 7. This document is the proof of that
> mapping. It is keyed on each prompt's ROADMAP MILESTONE field (the per-system milestone id its work
> implements) and cross-checked against the milestone headings of each system's Phase-6 roadmap under
> `../06-roadmaps/shared/` and `../06-roadmaps/subsystems/`. Prompt ids are the **global** `P-<NNN>` ids
> assigned by the master ledger (`README.md`); the per-system local ids (P-S01, REF-P1, EB-01, …) are recorded
> in the README's global-order table. Markdown only; no commits. Date: 2026-06-19.
>
> **How to read this.** One section per system, in the master build order (substrate → the M0/M1 dependency
> roots → the M2 reactive shared layer → the producers → the consumers). Each row is a roadmap milestone and
> the global prompt ids that implement it (in ascending global order). A milestone with no prompt would be a
> COVERAGE GAP; a prompt that maps to no milestone would be an ORPHAN. Both are flagged per-system; the verdict
> is at the foot.

---

## 1. Per-system coverage tables

### Platform Substrate (38 prompts)

Roadmap: `../06-roadmaps/shared/00-platform-substrate.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| SUB-M0 | P-001, P-002, P-003, P-004, P-005, P-006, P-008, P-009, P-010, P-017, P-018, P-029, P-030, P-031, P-032, P-033, P-034, P-035, P-036, P-037, P-038, P-039, P-040, P-041 |
| SUB-M1 | P-056, P-087, P-088 |
| SUB-M2 | P-135, P-136 |
| SUB-M4 | P-319, P-326 |
| SUB-M5 | P-433, P-434, P-435, P-436, P-437 |
| SUB-M6 | P-507, P-510 |

**Gaps:** none. **Orphans:** none.

### Event Bus (30 prompts)

Roadmap: `../06-roadmaps/shared/event-bus.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| B-M0 | P-011, P-012, P-013, P-014, P-015, P-019, P-042, P-043, P-044, P-045, P-046 |
| B-M1 | P-089, P-090, P-091, P-092, P-093 |
| B-M2 | P-137, P-138, P-139, P-140, P-141, P-142, P-143, P-144 |
| B-M3 | P-246 |
| B-M4 | P-327 |
| B-M5 | P-420, P-438, P-439, P-440 |

**Gaps:** none. **Orphans:** none.

### Identity & Access (35 prompts)

Roadmap: `../06-roadmaps/shared/identity-and-access.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| ID-M0 | P-022, P-023, P-024 |
| ID-M1 | P-054, P-057, P-064, P-065, P-066, P-067, P-068, P-069, P-070, P-071, P-072, P-073, P-074, P-075, P-076, P-077, P-078, P-079 |
| ID-M2 | P-133, P-134 |
| ID-M3/M4 | P-247, P-248, P-249, P-320, P-321, P-322, P-323 |
| ID-M5 | P-424, P-425, P-426, P-427, P-428 |

**Gaps:** none. **Orphans:** none.

### Tenancy & Control Plane (23 prompts)

Roadmap: `../06-roadmaps/shared/tenancy-and-control-plane.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| CP-M0 | P-025, P-026, P-027, P-028 |
| CP-M1 | P-080, P-081, P-082, P-083, P-084, P-085, P-086, P-096, P-097, P-098 |
| CP-M3 | P-250, P-251 |
| CP-M4 | P-324, P-325 |
| CP-M5 | P-429, P-430, P-431, P-432 |
| CP-M6 | P-508 |

**Gaps:** none. **Orphans:** none.

### Storage (37 prompts)

Roadmap: `../06-roadmaps/shared/storage.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| S-M0 | P-007, P-016, P-020, P-047, P-048 |
| S-M1 | P-058, P-059, P-060, P-061, P-094, P-095, P-099, P-100, P-101, P-102, P-103, P-104 |
| S-M2 | P-126, P-145, P-146, P-147 |
| S-M3 | P-252, P-253, P-254, P-255 |
| S-M4 | P-328, P-329, P-330, P-331 |
| S-M5 | P-441, P-442, P-443, P-444, P-445, P-446, P-447 |
| S-M6 | P-506 |

**Gaps:** none. **Orphans:** none.

### GDPR & Audit (38 prompts)

Roadmap: `../06-roadmaps/shared/gdpr-and-audit.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| GA-M0 | P-049, P-050, P-051 |
| GA-M1 | P-055, P-062, P-105, P-106, P-107, P-108, P-109, P-110, P-111, P-112, P-113, P-114, P-115, P-116, P-117, P-118, P-119 |
| GA-M2 | P-148, P-149, P-150, P-151, P-152, P-153 |
| GA-M3 | P-256, P-257 |
| GA-M4 | P-332, P-333, P-334 |
| GA-M5 | P-448, P-449, P-450, P-451, P-452 |
| GA-M6 | P-511, P-512 |

**Gaps:** none. **Orphans:** none.

### Reference Graph (29 prompts)

Roadmap: `../06-roadmaps/shared/reference-graph.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| R-M0 | P-052, P-053 |
| R-M1 | P-120, P-121 |
| R-M2 | P-154, P-155, P-156, P-157, P-158, P-159, P-160, P-161, P-162, P-163, P-164, P-165 |
| R-M3 | P-258, P-259 |
| R-M4 | P-335, P-336, P-337 |
| R-M5 | P-453, P-454, P-455, P-456, P-457, P-458 |
| R-M6 | P-513, P-514 |

**Gaps:** none. **Orphans:** none.

### Search & Indexing (33 prompts)

Roadmap: `../06-roadmaps/shared/search-and-indexing.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| S-M0 | P-021 |
| S-M1 | P-122 |
| S-M2 | P-166, P-167, P-168, P-169, P-170, P-171, P-172, P-173, P-174, P-175, P-176, P-177, P-178, P-179 |
| S-M3 | P-260, P-261, P-262 |
| S-M4 | P-338, P-339, P-340, P-341 |
| S-M5 | P-421, P-422, P-459, P-460, P-461, P-462, P-463, P-464, P-465 |
| S-M6 | P-515 |

**Gaps:** none. **Orphans:** none.

### Notifications (30 prompts)

Roadmap: `../06-roadmaps/shared/notifications.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| N-M2.0 | P-127, P-180, P-181, P-182 |
| N-M2.1 | P-183, P-184, P-185, P-186, P-187, P-188 |
| N-M2.2 | P-189, P-190, P-191 |
| N-M2.3 | P-192, P-193, P-194, P-195, P-196 |
| N-M3 | P-263, P-264 |
| N-M4 | P-342, P-343, P-344 |
| N-M5.1 | P-466 |
| N-M5.2 | P-467, P-468, P-469 |
| N-M5.3 | P-470, P-471, P-472 |

**Gaps:** none. **Orphans:** none.

### Durable Workflow (29 prompts)

Roadmap: `../06-roadmaps/shared/durable-workflow.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| FLOW-M2.1 | P-197, P-198, P-199, P-200, P-201, P-202, P-203, P-204 |
| FLOW-M2.2 | P-207, P-210 |
| FLOW-M2.3 | P-205, P-206, P-208, P-209 |
| FLOW-M2.4 | P-211, P-212, P-213, P-214, P-215 |
| FLOW-M3 | P-265, P-266 |
| FLOW-M4 | P-345, P-346 |
| FLOW-M5 | P-473, P-474, P-475, P-476, P-477 |
| FLOW-M6 | P-516 |

**Gaps:** none. **Orphans:** none.

### Agent Fabric (26 prompts)

Roadmap: `../06-roadmaps/shared/agent-fabric.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| M2-A | P-130, P-131, P-132, P-216 |
| M2-B | P-217, P-218, P-219, P-220, P-221, P-222, P-223, P-224, P-225, P-227 |
| M2-C | P-226, P-228, P-229 |
| M3 | P-267, P-268 |
| M4 | P-347, P-348 |
| M5 | P-478, P-479, P-480, P-481 |
| M6 | P-517 |

**Gaps:** none. **Orphans:** none.

### Git Hosting (35 prompts)

Roadmap: `../06-roadmaps/subsystems/git-hosting.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| Pre-work | P-063, P-123, P-124, P-230, P-231, P-232, P-233 |
| M3-G1 | P-269, P-270, P-271, P-272, P-273 |
| M3-G2 | P-274, P-275, P-276 |
| M3-G3 | P-277, P-278, P-279, P-280 |
| M3-G4 | P-281, P-282, P-284, P-285, P-286 |
| M3-G5 | P-287, P-288 |
| M3-G6 | P-283, P-289 |
| M3-G7 | P-290, P-291 |
| M3-G8 | P-292, P-293 |
| M5-G9 | P-482, P-483 |
| M6-G10 | P-518 |

**Gaps:** none. **Orphans:** none.

### Knowledge Platform (34 prompts)

Roadmap: `../06-roadmaps/subsystems/knowledge-platform.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| KN-M2 | P-234, P-235, P-236 |
| KN-M3a | P-294, P-295, P-296, P-297 |
| KN-M3b | P-298, P-299, P-300, P-301, P-302 |
| KN-M3c | P-303, P-304, P-305, P-306 |
| KN-M3d | P-307, P-308, P-309, P-310, P-311, P-312, P-313, P-314 |
| KN-M3e | P-315, P-316, P-317, P-318 |
| KN-M5 | P-484, P-485, P-486, P-487, P-488 |
| KN-M6 | P-519 |

**Gaps:** none. **Orphans:** none.

### Continuous Integration (35 prompts)

Roadmap: `../06-roadmaps/subsystems/continuous-integration.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| CI-M2 | P-129, P-237, P-238, P-239, P-240 |
| CI-M4 | P-349, P-350, P-351, P-352, P-353, P-354, P-355, P-356, P-357, P-358, P-359, P-360, P-361, P-362, P-363, P-364, P-365, P-366, P-367, P-368, P-369, P-370 |
| CI-M5 | P-423, P-489, P-490, P-491, P-492, P-493, P-494 |
| CI-M6 | P-509 |

**Gaps:** none. **Orphans:** none.

### Issue Tracker (37 prompts)

Roadmap: `../06-roadmaps/subsystems/issue-tracker.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| Pre-work | P-125, P-241, P-242, P-243 |
| M4-I1 | P-371, P-372, P-373, P-374, P-375, P-376 |
| M4-I2 | P-377, P-378 |
| M4-I3 | P-379, P-380, P-381, P-382, P-383 |
| M4-I4 | P-384, P-386, P-387 |
| M4-I5 | P-388, P-389 |
| M4-I6 | P-390, P-391, P-392 |
| M4-I7 | P-393, P-394, P-395, P-396 |
| M4-I8 | P-385, P-397 |
| M5-I9 | P-495, P-496, P-497, P-498, P-499 |
| M6-I10 | P-520 |

**Gaps:** none. **Orphans:** none.

### Chat (32 prompts)

Roadmap: `../06-roadmaps/subsystems/chat.md`

| Roadmap milestone | Prompt(s) |
|---|---|
| M2-C0 | P-128, P-244, P-245 |
| M4-C1 | P-398, P-399, P-400, P-401, P-402 |
| M4-C2 | P-403, P-404 |
| M4-C3 | P-405, P-406 |
| M4-C4 | P-407, P-408, P-409 |
| M4-C5 | P-410, P-412 |
| M4-C6 | P-413, P-414 |
| M4-C7 | P-415, P-416 |
| M4-C8 | P-411, P-417 |
| M4-C9 | P-418, P-419 |
| M5-C-S1 | P-500, P-501 |
| M5-C-X1 | P-504 |
| M5-C-S2 | P-502 |
| M5-C-X2 | P-505 |
| M5-C-S3 | P-503 |
| M6 | P-521 |

**Gaps:** none. **Orphans:** none.

---

## 2. Verdict

**COMPLETE.** Every roadmap milestone across all 16 systems maps to at least one prompt, and every one of the
521 prompts maps to exactly one primary roadmap milestone (no orphan). The mapping was derived two ways and
they agree: (a) bottom-up from each prompt's own ROADMAP MILESTONE field, and (b) top-down by enumerating every
milestone heading in each Phase-6 roadmap and confirming it appears here. No milestone heading in any roadmap is
left without a prompt.

**Notes on apparent (non-)gaps — each is a declared roadmap structure, not an omission:**

- **No M6 build prompt for several systems** (Event Bus B-M6, Identity ID-M6, Tenancy beyond CP-M6, etc.).
  Where a system's Phase-6 roadmap states "no new engine work in M6," the system has no dedicated M6 build
  prompt; its M6 obligation (the dogfood gate + the truth-up pass) is discharged by the shared/subsystem M6
  dogfood prompts (e.g. P-507, P-508, P-509, P-510..P-521). This is named in each roadmap, so it is not a
  silent drop. Event Bus and Identity carry their M6 work inside their M5/dogfood prompts; both roadmaps say so.
- **Band gaps in a system's milestone numbering** (Substrate has no SUB-M3; Tenancy has no CP-M2; Identity
  folds M3+M4 into one "ID-M3/M4" milestone). Each system only owns slices of the bands where it has work; the
  missing band number is genuine roadmap structure (the system does nothing in that band), not a hole.
- **Cross-band floor pairs are both present** (the name-your-floors discipline, master §5): the CAS floor (KN
  M3) → CRDT follow-on (KN-M5); local-disk git (M3-G1) → object-backed packs (M5-G9); single-cell (CP-M1) →
  multi-cell (CP-M5 + the EB-14 frame P-091 → EB-25 build P-438); fs-`BlobStore` (S-M1) → object-store
  (S-M5); pseudonymous commits (GIT M3) → audited history-rewrite (M5-G9); mock agent runtime (M2-A/B) →
  the real `LlmAgentRuntime` (post-M5). Each floor prompt and its follow-on prompt both appear, linked by the
  master sequencing §5 floor table.

## 3. Cross-system dependency integrity

**No unresolved cross-system dependency.** Every prompt-id-shaped token appearing in any prompt's DEPENDS-ON
field resolves to a real prompt in the ledger (verified by scanning all 521 DEPENDS-ON fields against the full
local-id set). No DEPENDS-ON points at a prompt that does not exist. The one soft reference that is explicitly
marked "not a hard edge" (P-GA-12's forward note to P-GA-20) is treated as a non-edge; the real edge runs
P-GA-20 → P-GA-12. The global order (README §2) places every depended-on prompt strictly before its dependent
(0 precedence violations), and the dependency graph is acyclic.
