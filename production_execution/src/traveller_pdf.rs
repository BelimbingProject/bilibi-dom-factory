//! Private work order travels with its saved routing and the recorded plan.
use crate::{
    contributions::FLOOR_VIEW,
    traveller::Traveller,
    traveller_layout::{CONTRACT, KEY},
    NewOrder, OrderKind, ProductionExecution,
};
use bilibi_artifacts::Artifacts;
use bilibi_branding::Branding;
use bilibi_database::{Error, Result};
use bilibi_module_registry::{
    contributions::{Access, Download as Route, DOWNLOADS},
    Contribution, Provenance,
};
use bilibi_settings::{Settings, Target};
use bilibi_templates::{CompiledPrint, PrintFacts, PrintValue, Templates};
use bilibi_tenancy::Scope;
use bilibi_ui::download::{Download, DownloadFuture, DownloadRequest, FileBody};
use std::sync::Arc;

pub(crate) const SERVICE: &str = "factory/production_execution.traveller_download";
const KIND: &str = "work-order-traveller";

pub(crate) struct TravellerDownload {
    pub(crate) production: Arc<ProductionExecution>,
    pub(crate) artifacts: Arc<Artifacts>,
    pub(crate) branding: Arc<Branding>,
    pub(crate) templates: Arc<Templates>,
    pub(crate) settings: Arc<Settings>,
}

struct PrintOrder<'a> {
    code: &'a str,
    kind: OrderKind,
    product_id: i64,
    formula_version: i32,
    routing_version: i32,
    demand_ref: Option<&'a str>,
}

pub(crate) fn validate(new: &NewOrder, plan: &Traveller) -> crate::Result<()> {
    let order = PrintOrder {
        code: &new.code,
        kind: new.kind,
        product_id: new.product_id,
        formula_version: new.formula_version,
        routing_version: new.routing_version,
        demand_ref: new.demand_ref.as_deref(),
    };
    let invalid = || crate::Error::Invalid {
        field: "Traveller",
        message: "exceeds the print contract or contains invalid facts",
    };
    for default in CONTRACT.defaults {
        let layout = CONTRACT
            .compile(default.language, default.layout)
            .map_err(|_| invalid())?;
        inline(
            &layout,
            &order,
            plan,
            default.language,
            bilibi_ui::Accent::default(),
        )
        .map_err(|_| invalid())?;
    }
    Ok(())
}

fn inline(
    layout: &CompiledPrint<'_>,
    order: &PrintOrder<'_>,
    plan: &Traveller,
    language: &str,
    accent: bilibi_ui::Accent,
) -> Result<bilibi_artifacts::InlineDocument> {
    let malay = language.split('-').next() == Some("ms");
    let kind = match (malay, order.kind) {
        (false, OrderKind::Order) => "Production order",
        (false, OrderKind::Batch) => "Production batch",
        (true, OrderKind::Order) => "Pesanan pengeluaran",
        (true, OrderKind::Batch) => "Kelompok pengeluaran",
    };
    let rows: Vec<_> = plan
        .operations
        .iter()
        .map(|operation| {
            let mut row = vec![
                ("sequence", PrintValue::Integer(operation.sequence)),
                ("operation", PrintValue::Text(operation.code.as_str())),
            ];
            if let Some(text) = operation.instructions.as_deref() {
                row.push(("instructions", PrintValue::Text(text)));
            }
            row
        })
        .collect();
    let rows: Vec<PrintFacts<'_>> = rows.iter().map(|row| row.as_slice()).collect();
    let mut facts = vec![
        ("number", PrintValue::Text(order.code)),
        ("kind", PrintValue::Text(kind)),
        ("issuer", PrintValue::Text(&plan.issuer)),
        ("product", PrintValue::Integer(order.product_id)),
        (
            "formula",
            PrintValue::Integer(i64::from(order.formula_version)),
        ),
        (
            "routing",
            PrintValue::Integer(i64::from(order.routing_version)),
        ),
        ("operations", PrintValue::Rows(&rows)),
    ];
    let quantity = plan
        .quantity
        .as_ref()
        .zip(plan.unit.as_ref())
        .map(|(quantity, unit)| format!("{quantity} {unit}"));
    if let Some(quantity) = quantity.as_deref() {
        facts.push(("quantity", PrintValue::Text(quantity)));
    }
    if let Some(due) = plan.due_on.as_deref() {
        facts.push((
            "due_on",
            PrintValue::Date(due.parse().map_err(|_| Error::Invalid)?),
        ));
    }
    if let Some(demand) = order.demand_ref.filter(|text| !text.trim().is_empty()) {
        facts.push(("demand", PrintValue::Text(demand)));
    }
    layout
        .document(order.kind.name(), &facts, accent)
        .map_err(|_| Error::Invalid)
}

impl Download<Scope, Error> for TravellerDownload {
    fn serve<'a>(&'a self, request: DownloadRequest<'a, Scope>) -> DownloadFuture<'a, Error> {
        Box::pin(async move {
            if request.operation != KIND {
                return Err(Error::NotFound);
            }
            let scope = request.scope;
            let company = scope.current_company().ok_or(Error::Forbidden)?;
            let id = request
                .path
                .get("id")
                .and_then(|id| id.parse::<i64>().ok())
                .filter(|id| *id > 0)
                .ok_or(Error::NotFound)?;
            let order = self
                .production
                .get_order(scope, company.get(), id)
                .await
                .map_err(|error| match error {
                    crate::Error::Access(error) => error,
                    crate::Error::OrderNotFound => Error::NotFound,
                    _ => Error::Internal,
                })?;
            let client = self.production.db.get().await?;
            let language = self
                .settings
                .get(
                    &**client,
                    scope,
                    Target::Company(company),
                    bilibi_templates::LANGUAGE,
                )
                .await?;
            let selected = self
                .templates
                .resolve_print(
                    &**client,
                    scope,
                    KEY,
                    Some(company),
                    language.value.as_str().unwrap_or("en"),
                    None,
                )
                .await?;
            let layout = CONTRACT
                .compile(&selected.language, &selected.layout)
                .map_err(|_| Error::Invalid)?;
            let accent = self.branding.accent(&**client).await?;
            let header = PrintOrder {
                code: &order.code,
                kind: order.kind,
                product_id: order.product_id,
                formula_version: order.formula_version,
                routing_version: order.routing_version,
                demand_ref: order.demand_ref.as_deref(),
            };
            let document = inline(
                &layout,
                &header,
                &order.traveller,
                &selected.language,
                accent,
            )?;
            let pdf = self
                .artifacts
                .print_document(&client, document)
                .await
                .map_err(artifact_error)?;
            Ok(FileBody::inline(
                pdf,
                "application/pdf",
                &format!("work-order-{id}.pdf"),
            ))
        })
    }
}

fn artifact_error(error: bilibi_artifacts::Error) -> Error {
    match error {
        bilibi_artifacts::Error::Database(error) => error,
        _ => Error::Internal,
    }
}

inventory::submit! { Contribution::new(DOWNLOADS, "factory/production_execution.traveller",
    Provenance { module:"factory/production_execution",source:file!(),line:line!() }, &Route {
        path:"/factory/orders/{id}/traveller.pdf",service:SERVICE,operation:KIND,access:Access::Capability(FLOOR_VIEW),
    }
) }
