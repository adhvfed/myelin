use super::*;

impl DurableGitBackend {
    fn pr_object_key(slug: &str, number: u64) -> String {
        format!("pr:{slug}:{number}")
    }

    fn thread_principal(tenant: &str, principal: &Principal) -> ThreadPrincipal {
        let kind = match principal.kind {
            PrincipalKind::Agent { .. } => PrincipalRole::Agent,
            PrincipalKind::Service => PrincipalRole::Service,
            _ => PrincipalRole::Human,
        };
        ThreadPrincipal::plain(kind, Self::pseudonym(tenant, principal))
    }

    fn require_pr(
        &self,
        loc: &RepoLoc,
        number: u64,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        self.pr_get(loc, number, principal)?
            .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))
    }

    fn resolve_thread_anchor(
        &self,
        loc: &RepoLoc,
        rec: &PrRecord,
        mut anchor: ThreadAnchor,
    ) -> Result<ThreadAnchor, DurableError> {
        let side = anchor
            .side
            .ok_or_else(|| DurableError::Git("anchor side is missing".into()))?;
        let line = anchor
            .line
            .and_then(|line| u32::try_from(line).ok())
            .filter(|line| *line > 0)
            .ok_or_else(|| DurableError::Git("anchor line is invalid".into()))?;
        let repo = self.store.open_repo(loc)?;
        let diff = repo
            .pr_diff(&rec.base_ref, &rec.head_oid, PR_DIFF_PER_FILE_LINE_CAP)?
            .ok_or_else(|| DurableError::Git("anchor diff is unavailable".into()))?;
        let resolved = diff.files.iter().any(|file| {
            let side_path = match side {
                AnchorSide::Old => file.old_path.as_deref().unwrap_or(file.path.as_str()),
                AnchorSide::New => file.path.as_str(),
            };
            let path_matches = side_path == anchor.path;
            path_matches
                && file.hunks.iter().any(|hunk| {
                    hunk.lines.iter().any(|candidate| match side {
                        AnchorSide::Old => candidate.old_no == Some(line),
                        AnchorSide::New => candidate.new_no == Some(line),
                    })
                })
        });
        if !resolved {
            return Err(DurableError::Git(
                "anchor path and line are not present in the current pull request diff".into(),
            ));
        }
        anchor.base_oid = Some(diff.base_oid);
        anchor.head_oid = Some(diff.head_oid);
        anchor.anchor_state = AnchorState::Live;
        Ok(anchor)
    }

    pub fn list_threads(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        principal: &Principal,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let doc = self.threads.load(&loc, &key)?;
        let viewer = Self::pseudonym(tenant, principal);
        Ok(viewed_threads_json(&doc.view_for(&viewer)))
    }

    pub fn create_thread(
        &self,
        target: PrActorContext<'_>,
        operation_nonce: &str,
        body: &Value,
    ) -> Result<Value, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
        let loc = Self::loc(tenant, region, slug);
        let rec = self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let body_md = require_body_md(body)?;
        let anchor = parse_anchor(body)?
            .map(|anchor| self.resolve_thread_anchor(&loc, &rec, anchor))
            .transpose()?;
        let author = Self::thread_principal(tenant, principal);
        let comment = CommentWrite::new(author, body_md, operation_nonce, now_unix())?;
        let outcome = self.threads.create_thread(&loc, &key, anchor, comment)?;
        if outcome.applied {
            self.bump_pr_updated(&loc, number, principal);
        }
        Ok(thread_json(&outcome.value))
    }

    pub fn add_thread_comment(
        &self,
        target: PrActorContext<'_>,
        thread_id: &str,
        operation_nonce: &str,
        body: &Value,
    ) -> Result<Value, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let body_md = require_body_md(body)?;
        let author = Self::thread_principal(tenant, principal);
        let comment = CommentWrite::new(author, body_md, operation_nonce, now_unix())?;
        let outcome = self.threads.add_comment(&loc, &key, thread_id, comment)?;
        if outcome.applied {
            self.bump_pr_updated(&loc, number, principal);
        }
        Ok(comment_json(&outcome.value))
    }

    pub fn resolve_thread(
        &self,
        target: PrActorContext<'_>,
        thread_id: &str,
        body: &Value,
    ) -> Result<Value, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let resolved = body
            .get("resolved")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        self.threads
            .resolve_thread(&loc, &key, thread_id, resolved)?;
        Ok(json!({ "thread_id": thread_id, "resolved": resolved }))
    }

    pub fn start_review_batch(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        principal: &Principal,
        operation_nonce: &str,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let reviewer = Self::thread_principal(tenant, principal);
        let batch = self
            .threads
            .start_review(&loc, &key, reviewer, operation_nonce)?;
        Ok(review_batch_json(&batch))
    }

    pub fn add_pending_comment(
        &self,
        target: PrActorContext<'_>,
        review_id: &str,
        operation_nonce: &str,
        body: &Value,
    ) -> Result<Value, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
        let loc = Self::loc(tenant, region, slug);
        let rec = self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let body_md = require_body_md(body)?;
        let anchor = parse_anchor(body)?
            .map(|anchor| self.resolve_thread_anchor(&loc, &rec, anchor))
            .transpose()?;
        let author = Self::thread_principal(tenant, principal);
        let comment = CommentWrite::new(author, body_md, operation_nonce, now_unix())?;
        let request = PendingCommentRequest::new(loc, key, review_id, anchor, comment)?;
        let comment = self.threads.add_pending_comment(request)?;
        Ok(comment_json(&comment))
    }

    pub fn submit_review_batch(
        &self,
        target: PrActorContext<'_>,
        review_id: &str,
        body: &Value,
        operation_nonce: &str,
        operation_id: &PrOperationId,
    ) -> Result<Value, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let verdict = match body.get("verdict").and_then(Value::as_str) {
            Some("approved") | Some("approve") => BatchVerdict::Approved,
            Some("changes_requested") | Some("request-changes") | Some("request_changes") => {
                BatchVerdict::ChangesRequested
            }
            Some("commented") | Some("comment") | None => BatchVerdict::Commented,
            Some(other) => {
                return Err(DurableError::Git(format!(
                    "unknown review verdict `{other}`"
                )))
            }
        };
        let summary_md = body
            .get("summary_md")
            .and_then(Value::as_str)
            .map(str::to_string);
        let actor = Self::thread_principal(tenant, principal);
        let decision = ReviewDecision::new(verdict, summary_md)?;
        let request = SubmitReviewRequest::new(
            loc.clone(),
            key,
            review_id,
            actor,
            decision,
            operation_nonce,
            now_unix(),
        )?;
        let submitted = self.threads.submit_review(request)?;
        // Production PR mutations have their own command ledger, so replaying this projection is
        // how a retry repairs a failure after the conversation document was already committed. The
        // in-memory test backend has no such ledger and must only project the first application.
        let reconcile_projection = submitted.applied || self.pg_prs.is_some();
        if let Some(ref batch) = submitted.value {
            if !batch.review.advisory {
                let gate_verdict = match verdict {
                    BatchVerdict::Approved => Some("approve"),
                    BatchVerdict::ChangesRequested => Some("request-changes"),
                    _ => None,
                };
                if let Some(v) = gate_verdict.filter(|_| reconcile_projection) {
                    self.submit_review_with_operation(
                        RepoActorContext::new(tenant, region, slug, principal).for_pr(number),
                        v,
                        operation_id,
                    )?;
                }
            }
        }
        if submitted
            .value
            .as_ref()
            .is_some_and(|batch| batch.review.advisory || verdict == BatchVerdict::Commented)
        {
            let projection_operation = PrOperationId::derive(
                "myelin.git.review-batch-projection.v1",
                &[operation_id.digest().as_bytes()],
            )?;
            if reconcile_projection {
                self.pr_mutate(
                    &loc,
                    number,
                    PrMutation::Touch,
                    &projection_operation,
                    principal,
                )?;
            }
        }
        Ok(json!({
            "emitted": submitted.value.is_some(),
            "review": submitted.value.as_ref().map(|b| review_batch_json(&b.review)),
            "comment_ids": submitted.value
                .as_ref()
                .map(|b| b.comment_ids.clone())
                .unwrap_or_default(),
        }))
    }

    pub fn discard_review_batch(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        review_id: &str,
        principal: &Principal,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let actor = Self::thread_principal(tenant, principal);
        self.threads.discard_review(&loc, &key, review_id, &actor)?;
        Ok(json!({ "discarded": review_id }))
    }

    fn bump_pr_updated(&self, loc: &RepoLoc, number: u64, principal: &Principal) {
        if let Ok(operation_id) = self.fresh_operation_id() {
            let _ = self.pr_mutate(loc, number, PrMutation::Touch, &operation_id, principal);
        }
    }
}
