use std::collections::BTreeMap;

use myelin_query::{FieldId, FieldType, FieldValue};
use myelin_storage::blob::{BlobError, BlobStore, ContentHash};
use myelin_storage::migration::MigrationPhase;
use myelin_tenancy::TenantId;

use crate::database::{FacetIndexHint, FacetPath};
use crate::rollup::{MaterialisationHint, RollupFn};

pub const DB_ROW_TABLE: &str = "db_row";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacetPromotionStep {
    pub phase: MigrationPhase,
    pub ddl: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacetPromotionPlan {
    pub field_id: FieldId,
    pub field_type: FieldType,
    pub personal_data: bool,
    pub steps: Vec<FacetPromotionStep>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FacetPromotionError {
    PiiFacetGated { field: String },
}

impl std::fmt::Display for FacetPromotionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FacetPromotionError::PiiFacetGated { field } => write!(
                f,
                "facet `{field}` carries personal data - its generated column is gated behind the \
                 field-level caveat (contract 10.2); promotion refused without the cleared caveat"
            ),
        }
    }
}

impl std::error::Error for FacetPromotionError {}

fn generated_column_type(field_type: FieldType) -> &'static str {
    match field_type {
        FieldType::Int => "BIGINT",
        FieldType::Bool => "BOOLEAN",
        FieldType::Text
        | FieldType::Date
        | FieldType::Select
        | FieldType::Relation
        | FieldType::Principal
        | FieldType::OrderKey => "TEXT",
    }
}

fn generated_column_expr(field: &str, field_type: FieldType) -> String {
    let path = format!("props ->> '{}'", sanitize_ident(field));
    match field_type {
        FieldType::Int => format!("({path})::BIGINT"),
        FieldType::Bool => format!("({path})::BOOLEAN"),
        _ => path,
    }
}

fn sanitize_ident(field: &str) -> String {
    field
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

pub fn promote_facet(hint: &FacetIndexHint) -> Result<FacetPromotionPlan, FacetPromotionError> {
    if hint.personal_data {
        return Err(FacetPromotionError::PiiFacetGated {
            field: hint.field_id.to_string(),
        });
    }
    Ok(build_facet_plan(hint))
}

pub fn promote_facet_pii_cleared(hint: &FacetIndexHint) -> FacetPromotionPlan {
    build_facet_plan(hint)
}

fn build_facet_plan(hint: &FacetIndexHint) -> FacetPromotionPlan {
    let col = format!("{}__col", sanitize_ident(hint.field_id.as_str()));
    let idx = format!("{DB_ROW_TABLE}_{col}_idx");
    let sql_type = generated_column_type(hint.field_type);
    let expr = generated_column_expr(hint.field_id.as_str(), hint.field_type);
    let steps = vec![
        FacetPromotionStep {
            phase: MigrationPhase::Expand,
            ddl: format!(
                "ALTER TABLE {DB_ROW_TABLE} ADD COLUMN IF NOT EXISTS {col} {sql_type} \
                 GENERATED ALWAYS AS ({expr}) STORED; \
                 CREATE INDEX CONCURRENTLY IF NOT EXISTS {idx} ON {DB_ROW_TABLE} ({col})"
            ),
        },
        FacetPromotionStep {
            phase: MigrationPhase::Backfill,
            ddl: format!(
                "/* backfill {col}: STORED generated column materialised by the engine; \
                 resumable no-op batch anchor over {DB_ROW_TABLE} (idempotent) */"
            ),
        },
        FacetPromotionStep {
            phase: MigrationPhase::Contract,
            ddl: format!("ANALYZE {DB_ROW_TABLE} ({col})"),
        },
    ];
    FacetPromotionPlan {
        field_id: hint.field_id.clone(),
        field_type: hint.field_type,
        personal_data: hint.personal_data,
        steps,
    }
}

impl FacetPromotionPlan {
    pub fn installed_path(&self) -> FacetPath {
        FacetPath::GeneratedColumn
    }

    pub fn phases(&self) -> Vec<MigrationPhase> {
        self.steps.iter().map(|s| s.phase).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowUpdatedDelta {
    pub src_row: String,
    pub target_row: String,
    pub old_value: Option<i64>,
    pub new_value: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct MaterialisedRollup {
    aggregates: BTreeMap<(String, String, String), RollupAggState>,
    func: RollupFn,
    db_id: String,
    field: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RollupAggState {
    values: BTreeMap<String, i64>,
}

impl MaterialisedRollup {
    pub fn for_hint(hint: &MaterialisationHint, func: RollupFn) -> MaterialisedRollup {
        MaterialisedRollup {
            aggregates: BTreeMap::new(),
            func,
            db_id: hint.db_id.clone(),
            field: hint.field.to_string(),
        }
    }

    pub fn apply_delta(&mut self, delta: &RowUpdatedDelta) {
        let key = (
            self.db_id.clone(),
            self.field.clone(),
            delta.src_row.clone(),
        );
        let state = self.aggregates.entry(key).or_default();
        match delta.new_value {
            Some(v) => {
                state.values.insert(delta.target_row.clone(), v);
            }
            None => {
                state.values.remove(&delta.target_row);
            }
        }
    }

    pub fn read(&self, src_row: &str) -> MaterialisedValue {
        let key = (self.db_id.clone(), self.field.clone(), src_row.to_string());
        let values: Vec<i64> = self
            .aggregates
            .get(&key)
            .map(|s| s.values.values().copied().collect())
            .unwrap_or_default();
        aggregate(self.func, &values)
    }

    pub fn materialised_rows(&self) -> usize {
        self.aggregates.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialisedValue {
    Int(i64),
    Empty,
}

fn aggregate(func: RollupFn, values: &[i64]) -> MaterialisedValue {
    match func {
        RollupFn::Count => MaterialisedValue::Int(values.len() as i64),
        RollupFn::Sum => MaterialisedValue::Int(values.iter().sum()),
        RollupFn::Avg => {
            if values.is_empty() {
                MaterialisedValue::Int(0)
            } else {
                let sum: i64 = values.iter().sum();
                MaterialisedValue::Int(sum / values.len() as i64)
            }
        }
        RollupFn::Min => match values.iter().min() {
            Some(m) => MaterialisedValue::Int(*m),
            None => MaterialisedValue::Empty,
        },
        RollupFn::Max => match values.iter().max() {
            Some(m) => MaterialisedValue::Int(*m),
            None => MaterialisedValue::Empty,
        },
    }
}

pub fn read_time_recompute(func: RollupFn, visible_values: &[i64]) -> MaterialisedValue {
    aggregate(func, visible_values)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobParityVerdict {
    pub fs_address: ContentHash,
    pub object_address: ContentHash,
    pub byte_identical: bool,
}

pub fn materialise_blob_store_parity<F, O>(
    fs: &F,
    object: &O,
    tenant: &TenantId,
    bytes: &[u8],
) -> Result<BlobParityVerdict, BlobError>
where
    F: BlobStore,
    O: BlobStore,
{
    let fs_address = fs.put(tenant, bytes)?;
    let object_address = object.put(tenant, bytes)?;
    let fs_back = fs.get(tenant, &fs_address)?;
    let object_back = object.get(tenant, &object_address)?;
    let address_identical = fs_address == object_address;
    let fs_roundtrip_ok = fs_back == bytes;
    let object_roundtrip_ok = object_back == bytes;
    let byte_identical = address_identical && fs_roundtrip_ok && object_roundtrip_ok;
    Ok(BlobParityVerdict {
        fs_address,
        object_address,
        byte_identical,
    })
}

pub fn target_numeric_value(value: Option<&FieldValue>) -> Option<i64> {
    match value {
        Some(FieldValue::Int(n)) => Some(*n),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
