//! Work instructions frozen with the order's exact published routing.
use crate::{Error, ProductionExecution, Result};
use bilibi_factory_inventory::ProductionImport;
use bilibi_factory_product_definition::SelectedRevisions;
use bilibi_tenancy::Scope;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tokio_postgres::Transaction;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkPlan {
    pub target_quantity: Option<String>,
    pub target_unit_id: Option<i64>,
    pub due_on: Option<String>,
    #[serde(default)]
    pub instructions: Vec<WorkInstruction>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkInstruction {
    pub operation: String,
    pub instructions: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Traveller {
    pub issuer: String,
    pub quantity: Option<String>,
    pub unit: Option<String>,
    pub due_on: Option<String>,
    pub operations: Vec<TravellerStep>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct TravellerStep {
    pub sequence: i64,
    pub code: String,
    pub instructions: Option<String>,
}

fn invalid(field: &'static str, message: &'static str) -> Error {
    Error::Invalid { field, message }
}

impl ProductionExecution {
    pub(crate) async fn freeze_traveller(
        &self,
        client: &Transaction<'_>,
        scope: &Scope,
        company: i64,
        import: Option<&ProductionImport<'_>>,
        picked: SelectedRevisions,
        plan: &WorkPlan,
    ) -> Result<Traveller> {
        let issuer = bilibi_company::identity(client, scope, company)
            .await?
            .ok_or(bilibi_database::Error::NotFound)?;
        let (quantity, unit) = match (&plan.target_quantity, plan.target_unit_id) {
            (None, None) => (None, None),
            (Some(quantity), Some(id)) => {
                let text = quantity.trim();
                let valid: bool = client
                    .query_one(
                        "SELECT CASE WHEN $1 ~ '^[0-9]{1,24}([.][0-9]{1,12})?$'
                                       THEN $1::numeric > 0 ELSE false END",
                        &[&text],
                    )
                    .await?
                    .get(0);
                if !valid {
                    return Err(invalid("Target quantity", "must be a positive decimal"));
                }
                let inventory = &self.completer.inventory;
                let unit = match import {
                    Some(import) => inventory.get_import_unit_on(client, import, id).await?,
                    None => inventory.get_unit_on(client, scope, company, id).await?,
                };
                if unit.retired {
                    return Err(invalid("Target unit", "must be active"));
                }
                (Some(text.to_owned()), Some(unit.code))
            }
            _ => {
                return Err(invalid(
                    "Target quantity",
                    "and unit must be provided together",
                ))
            }
        };
        let due_on = plan
            .due_on
            .as_deref()
            .map(|date| {
                date.parse::<jiff::civil::Date>()
                    .map(|date| date.to_string())
                    .map_err(|_| invalid("Due date", "must be a calendar date"))
            })
            .transpose()?;
        let known: BTreeSet<_> = picked
            .routing
            .operations
            .iter()
            .map(|op| op.code.as_str())
            .collect();
        let mut notes = BTreeMap::new();
        for item in &plan.instructions {
            let instructions = item.instructions.trim();
            if instructions.is_empty() || instructions.len() > 12_000 || instructions.contains('\0')
            {
                return Err(invalid(
                    "Instructions",
                    "must contain between 1 and 12,000 bytes",
                ));
            }
            if !known.contains(item.operation.as_str())
                || notes
                    .insert(item.operation.as_str(), instructions)
                    .is_some()
            {
                return Err(invalid(
                    "Instructions",
                    "must name each published routing operation at most once",
                ));
            }
        }
        let mut operations: Vec<_> = picked
            .routing
            .operations
            .into_iter()
            .map(|operation| {
                let instructions = notes
                    .get(operation.code.as_str())
                    .map(|text| (*text).to_owned());
                TravellerStep {
                    sequence: operation.sequence,
                    code: operation.code,
                    instructions,
                }
            })
            .collect();
        operations.sort_by_key(|operation| operation.sequence);
        Ok(Traveller {
            issuer: issuer.display_name,
            quantity,
            unit,
            due_on,
            operations,
        })
    }
}
