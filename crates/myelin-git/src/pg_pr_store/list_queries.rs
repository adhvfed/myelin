use crate::durable::DurableError;
use crate::pr_list_pagination::{PrListDirection, PrListPage};
use crate::pr_store::{
    PrCrossListQuery, PrListBucket, PrListQuery, PrListSort, PrListState, PR_LIST_OFFSET_MAX,
};

use super::PR_RECORD_COLUMNS;

fn pr_list_state_predicate(state: PrListState) -> &'static str {
    match state {
        PrListState::Open => "g.record->>'state' IN ('Open','Draft')",
        PrListState::Merged => "g.record->>'state' = 'Merged'",
        PrListState::Closed => "g.record->>'state' = 'Closed'",
        PrListState::All => "TRUE",
    }
}

pub(super) fn pr_list_page_sql(query: &PrListQuery) -> String {
    let canonical_page_order = match query.sort {
        PrListSort::Created => "g.number DESC",
        PrListSort::Updated => "((g.record->>'updated_at')::bigint) DESC NULLS LAST, g.number DESC",
    };
    let (cursor_predicate, scan_order, page_suffix) = match &query.page {
        PrListPage::Initial => (
            "TRUE".to_string(),
            canonical_page_order.to_string(),
            "LIMIT $5",
        ),
        PrListPage::LegacyOffset(_) => (
            "TRUE".to_string(),
            canonical_page_order.to_string(),
            "LIMIT $6 OFFSET $5",
        ),
        PrListPage::Keyset(cursor) => {
            let newer = cursor.direction() == PrListDirection::Newer;
            let predicate = match (query.sort, cursor.direction()) {
                (PrListSort::Created, PrListDirection::Older) => {
                    "$5::bigint IS NULL AND g.number < $6"
                }
                (PrListSort::Created, PrListDirection::Newer) => {
                    "$5::bigint IS NULL AND g.number > $6"
                }
                (PrListSort::Updated, PrListDirection::Older) => {
                    "(($5::bigint IS NOT NULL AND (((g.record->>'updated_at')::bigint) < $5 OR (g.record->>'updated_at') IS NULL OR (((g.record->>'updated_at')::bigint) = $5 AND g.number < $6))) OR ($5::bigint IS NULL AND (g.record->>'updated_at') IS NULL AND g.number < $6))"
                }
                (PrListSort::Updated, PrListDirection::Newer) => {
                    "(($5::bigint IS NOT NULL AND (((g.record->>'updated_at')::bigint) > $5 OR (((g.record->>'updated_at')::bigint) = $5 AND g.number > $6))) OR ($5::bigint IS NULL AND ((g.record->>'updated_at') IS NOT NULL OR ((g.record->>'updated_at') IS NULL AND g.number > $6))))"
                }
            };
            let reverse = match query.sort {
                PrListSort::Created => "g.number ASC",
                PrListSort::Updated => {
                    "((g.record->>'updated_at')::bigint) ASC NULLS FIRST, g.number ASC"
                }
            };
            (
                predicate.to_string(),
                if newer { reverse } else { canonical_page_order }.to_string(),
                "LIMIT $7",
            )
        }
    };
    let output_order = match query.sort {
        PrListSort::Created => "p.page_number DESC NULLS LAST",
        PrListSort::Updated => "p.page_updated_at DESC NULLS LAST, p.page_number DESC NULLS LAST",
    };
    let reverse_output_order = match query.sort {
        PrListSort::Created => "p.page_number ASC",
        PrListSort::Updated => "p.page_updated_at ASC NULLS FIRST, p.page_number ASC",
    };
    let (has_newer_predicate, has_older_predicate) = match query.sort {
        PrListSort::Created => ("x.number > f.page_number", "x.number < l.page_number"),
        PrListSort::Updated => (
            "((f.page_updated_at IS NOT NULL AND (((x.record->>'updated_at')::bigint) > f.page_updated_at OR (((x.record->>'updated_at')::bigint) = f.page_updated_at AND x.number > f.page_number))) OR (f.page_updated_at IS NULL AND ((x.record->>'updated_at') IS NOT NULL OR ((x.record->>'updated_at') IS NULL AND x.number > f.page_number))))",
            "((l.page_updated_at IS NOT NULL AND (((x.record->>'updated_at')::bigint) < l.page_updated_at OR (x.record->>'updated_at') IS NULL OR (((x.record->>'updated_at')::bigint) = l.page_updated_at AND x.number < l.page_number))) OR (l.page_updated_at IS NULL AND (x.record->>'updated_at') IS NULL AND x.number < l.page_number))",
        ),
    };
    format!(
        "WITH counts AS (\
           SELECT count(*) FILTER (WHERE g.record->>'state' IN ('Open','Draft'))::bigint AS open_count, \
                  count(*) FILTER (WHERE g.record->>'state' = 'Merged')::bigint AS merged_count, \
                  count(*) FILTER (WHERE g.record->>'state' = 'Closed')::bigint AS closed_count, \
                  count(*)::bigint AS all_count, \
                  count(*) FILTER (WHERE g.record->>'author_pseudonym' = $4)::bigint AS yours_count, \
                  count(*) FILTER (WHERE (\
                    SELECT review.item->>'state' \
                      FROM jsonb_array_elements(COALESCE(g.record->'reviews','[]'::jsonb)) \
                           WITH ORDINALITY AS review(item, position) \
                     WHERE review.item->>'reviewer_pseudonym' = $4 \
                     ORDER BY review.position DESC LIMIT 1\
                  ) = 'Requested')::bigint AS needs_review_count \
             FROM git_pr g \
            WHERE g.tenant_id=$1 AND g.region=$2 AND g.repo_slug=$3\
         ), page_rows AS (\
           SELECT g.{columns}, g.number AS page_number, \
                  (g.record->>'updated_at')::bigint AS page_updated_at \
             FROM git_pr g \
            WHERE g.tenant_id=$1 AND g.region=$2 AND g.repo_slug=$3 \
              AND ({state}) AND ({cursor_predicate}) \
            ORDER BY {scan_order} {page_suffix}\
         ), first_row AS (\
           SELECT p.page_number,p.page_updated_at FROM page_rows p ORDER BY {output_order} LIMIT 1\
         ), last_row AS (\
           SELECT p.page_number,p.page_updated_at FROM page_rows p ORDER BY {reverse_output_order} LIMIT 1\
         ), flags AS (\
           SELECT EXISTS(SELECT 1 FROM git_pr x CROSS JOIN first_row f \
                          WHERE x.tenant_id=$1 AND x.region=$2 AND x.repo_slug=$3 \
                            AND ({x_state}) AND ({has_newer_predicate})) AS has_newer, \
                  EXISTS(SELECT 1 FROM git_pr x CROSS JOIN last_row l \
                          WHERE x.tenant_id=$1 AND x.region=$2 AND x.repo_slug=$3 \
                            AND ({x_state}) AND ({has_older_predicate})) AS has_older\
         ) \
         SELECT c.open_count,c.merged_count,c.closed_count,c.all_count,c.yours_count,\
                c.needs_review_count,f.has_newer,f.has_older,p.page_number,p.page_updated_at,\
                p.record,p.head_repo_slug,p.title_nonce,p.title_ciphertext,p.title_pii_key_ref,\
                p.body_nonce,p.body_ciphertext,p.body_pii_key_ref,p.author_subject_id \
           FROM counts c CROSS JOIN flags f LEFT JOIN page_rows p ON TRUE ORDER BY {output_order}",
        columns = PR_RECORD_COLUMNS,
        state = pr_list_state_predicate(query.state),
        x_state = pr_list_state_predicate(query.state).replace("g.", "x."),
    )
}

fn pr_cross_bucket_predicate(bucket: PrListBucket) -> &'static str {
    match bucket {
        PrListBucket::Yours => "g.record->>'author_pseudonym' = $4",
        PrListBucket::NeedsReview => {
            "g.record->>'author_pseudonym' <> $4 \
             AND g.record->>'state' IN ('Open','Draft') \
             AND (SELECT review.item->>'state' \
                    FROM jsonb_array_elements(COALESCE(g.record->'reviews','[]'::jsonb)) \
                         WITH ORDINALITY AS review(item, position) \
                   WHERE review.item->>'reviewer_pseudonym' = $4 \
                   ORDER BY review.position DESC LIMIT 1) = 'Requested'"
        }
    }
}

pub(super) fn pr_cross_list_page_sql(query: &PrCrossListQuery) -> String {
    let canonical_page_order = match query.sort {
        PrListSort::Created => "g.number DESC, g.repo_slug ASC",
        PrListSort::Updated => {
            "((g.record->>'updated_at')::bigint) DESC NULLS LAST, g.number DESC, g.repo_slug ASC"
        }
    };
    let (cursor_predicate, scan_order, page_suffix) = match &query.page {
        PrListPage::Initial => (
            "TRUE".to_string(),
            canonical_page_order.to_string(),
            "LIMIT $5",
        ),
        PrListPage::LegacyOffset(_) => (
            "TRUE".to_string(),
            canonical_page_order.to_string(),
            "LIMIT $6 OFFSET $5",
        ),
        PrListPage::Keyset(cursor) => {
            let newer = cursor.direction() == PrListDirection::Newer;
            let predicate = match (query.sort, cursor.direction()) {
                (PrListSort::Created, PrListDirection::Older) => "$5::bigint IS NULL AND (g.number < $6 OR (g.number = $6 AND g.repo_slug > $7))",
                (PrListSort::Created, PrListDirection::Newer) => "$5::bigint IS NULL AND (g.number > $6 OR (g.number = $6 AND g.repo_slug < $7))",
                (PrListSort::Updated, PrListDirection::Older) => "(($5::bigint IS NOT NULL AND (((g.record->>'updated_at')::bigint) < $5 OR (g.record->>'updated_at') IS NULL OR (((g.record->>'updated_at')::bigint) = $5 AND (g.number < $6 OR (g.number = $6 AND g.repo_slug > $7))))) OR ($5::bigint IS NULL AND (g.record->>'updated_at') IS NULL AND (g.number < $6 OR (g.number = $6 AND g.repo_slug > $7))))",
                (PrListSort::Updated, PrListDirection::Newer) => "(($5::bigint IS NOT NULL AND (((g.record->>'updated_at')::bigint) > $5 OR (((g.record->>'updated_at')::bigint) = $5 AND (g.number > $6 OR (g.number = $6 AND g.repo_slug < $7))))) OR ($5::bigint IS NULL AND ((g.record->>'updated_at') IS NOT NULL OR ((g.record->>'updated_at') IS NULL AND (g.number > $6 OR (g.number = $6 AND g.repo_slug < $7))))))",
            };
            let reverse = match query.sort {
                PrListSort::Created => "g.number ASC, g.repo_slug DESC",
                PrListSort::Updated => "((g.record->>'updated_at')::bigint) ASC NULLS FIRST, g.number ASC, g.repo_slug DESC",
            };
            (
                predicate.to_string(),
                if newer { reverse } else { canonical_page_order }.to_string(),
                "LIMIT $8",
            )
        }
    };
    let output_order = match query.sort {
        PrListSort::Created => "p.page_number DESC NULLS LAST, p.page_repo_slug ASC NULLS LAST",
        PrListSort::Updated => {
            "p.page_updated_at DESC NULLS LAST, p.page_number DESC NULLS LAST, \
             p.page_repo_slug ASC NULLS LAST"
        }
    };
    let reverse_output_order = match query.sort {
        PrListSort::Created => "p.page_number ASC, p.page_repo_slug DESC",
        PrListSort::Updated => {
            "p.page_updated_at ASC NULLS FIRST, p.page_number ASC, p.page_repo_slug DESC"
        }
    };
    let (has_newer_predicate, has_older_predicate) = match query.sort {
        PrListSort::Created => (
            "x.number > f.page_number OR (x.number = f.page_number AND x.repo_slug < f.page_repo_slug)",
            "x.number < l.page_number OR (x.number = l.page_number AND x.repo_slug > l.page_repo_slug)",
        ),
        PrListSort::Updated => (
            "((f.page_updated_at IS NOT NULL AND (((x.record->>'updated_at')::bigint) > f.page_updated_at OR (((x.record->>'updated_at')::bigint) = f.page_updated_at AND (x.number > f.page_number OR (x.number = f.page_number AND x.repo_slug < f.page_repo_slug))))) OR (f.page_updated_at IS NULL AND ((x.record->>'updated_at') IS NOT NULL OR ((x.record->>'updated_at') IS NULL AND (x.number > f.page_number OR (x.number = f.page_number AND x.repo_slug < f.page_repo_slug))))))",
            "((l.page_updated_at IS NOT NULL AND (((x.record->>'updated_at')::bigint) < l.page_updated_at OR (x.record->>'updated_at') IS NULL OR (((x.record->>'updated_at')::bigint) = l.page_updated_at AND (x.number < l.page_number OR (x.number = l.page_number AND x.repo_slug > l.page_repo_slug))))) OR (l.page_updated_at IS NULL AND (x.record->>'updated_at') IS NULL AND (x.number < l.page_number OR (x.number = l.page_number AND x.repo_slug > l.page_repo_slug))))",
        ),
    };
    let predicate = pr_cross_bucket_predicate(query.bucket);
    format!(
        "WITH counts AS (\
           SELECT count(*)::bigint AS bucket_count FROM git_pr g \
            WHERE g.tenant_id=$1 AND g.region=$2 AND g.repo_slug = ANY($3) AND ({predicate})\
         ), page_rows AS (\
           SELECT g.repo_slug AS page_repo_slug, g.{columns}, g.number AS page_number, \
                  (g.record->>'updated_at')::bigint AS page_updated_at FROM git_pr g \
            WHERE g.tenant_id=$1 AND g.region=$2 AND g.repo_slug = ANY($3) AND ({predicate}) \
              AND ({cursor_predicate}) \
            ORDER BY {scan_order} {page_suffix}\
         ), first_row AS (\
           SELECT p.page_repo_slug,p.page_number,p.page_updated_at FROM page_rows p ORDER BY {output_order} LIMIT 1\
         ), last_row AS (\
           SELECT p.page_repo_slug,p.page_number,p.page_updated_at FROM page_rows p ORDER BY {reverse_output_order} LIMIT 1\
         ), flags AS (\
           SELECT EXISTS(SELECT 1 FROM git_pr x CROSS JOIN first_row f \
                          WHERE x.tenant_id=$1 AND x.region=$2 AND x.repo_slug = ANY($3) \
                            AND ({x_predicate}) AND ({has_newer_predicate})) AS has_newer, \
                  EXISTS(SELECT 1 FROM git_pr x CROSS JOIN last_row l \
                          WHERE x.tenant_id=$1 AND x.region=$2 AND x.repo_slug = ANY($3) \
                            AND ({x_predicate}) AND ({has_older_predicate})) AS has_older\
         ) SELECT c.bucket_count,f.has_newer,f.has_older,p.page_repo_slug,p.page_number,p.page_updated_at,\
                p.record,p.head_repo_slug,p.title_nonce,p.title_ciphertext,p.title_pii_key_ref,\
                p.body_nonce,p.body_ciphertext,p.body_pii_key_ref,p.author_subject_id \
           FROM counts c CROSS JOIN flags f LEFT JOIN page_rows p ON TRUE ORDER BY {output_order}",
        columns = PR_RECORD_COLUMNS,
        x_predicate = predicate.replace("g.", "x."),
    )
}

pub(super) fn validate_cross_visible_slugs(slugs: &[String]) -> Result<(), DurableError> {
    if slugs.len() > PR_LIST_OFFSET_MAX {
        return Err(DurableError::Git(
            "cross-repository PR visible set exceeds 10000 repositories".into(),
        ));
    }
    let mut unique = std::collections::BTreeSet::new();
    for slug in slugs {
        crate::coordinate::RepositorySlug::parse(slug).map_err(|_| {
            DurableError::Git("cross-repository PR visible set contains invalid slug".into())
        })?;
        if !unique.insert(slug) {
            return Err(DurableError::Git(
                "cross-repository PR visible set contains duplicate slug".into(),
            ));
        }
    }
    Ok(())
}
