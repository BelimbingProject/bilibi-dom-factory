//! Private work order travels with its saved routing and the recorded plan.
use crate::{
    contributions::FLOOR_VIEW,
    orders,
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
use bilibi_party_ids::OrganisationId;
use bilibi_settings::{Settings, Target};
use bilibi_templates::{CompiledPrint, Origin, PrintFacts, PrintValue, Problem, Templates};
use bilibi_tenancy::{ReadClient, Scope};
use bilibi_ui::download::{Download, DownloadFuture, DownloadRequest, FileBody};
use std::sync::Arc;

pub(crate) const SERVICE: &str = "factory/production_execution.traveller_download";
const KIND: &str = "work-order-traveller";

pub(crate) struct TravellerDownload {
    pub(crate) production: Arc<ProductionExecution>,
    pub(crate) artifacts: Arc<Artifacts>,
    pub(crate) branding: Arc<Branding>,
}

pub(crate) struct TravellerLayouts {
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

impl TravellerLayouts {
    pub(crate) async fn validate(
        &self,
        client: &dyn ReadClient,
        scope: &Scope,
        company: i64,
        new: &NewOrder,
        plan: &Traveller,
    ) -> crate::Result<()> {
        let order = PrintOrder {
            code: &new.code,
            kind: new.kind,
            product_id: new.product_id,
            formula_version: new.formula_version,
            routing_version: new.routing_version,
            demand_ref: new.demand_ref.as_deref(),
        };
        let accent = bilibi_ui::Accent::default();
        let invalid = || crate::Error::Invalid {
            field: "Traveller",
            message: "exceeds the print contract or contains invalid facts",
        };
        for default in CONTRACT.defaults {
            let layout = CONTRACT
                .compile(default.language, default.layout)
                .map_err(|_| invalid())?;
            inline(&layout, &order, plan, default.language, accent).map_err(|_| invalid())?;
        }
        let company = OrganisationId::new(company).map_err(|_| Error::Invalid)?;
        self.document(client, scope, company, &order, plan, accent)
            .await
            .map_err(|error| match error {
                Error::Invalid => invalid(),
                error => error.into(),
            })?;
        Ok(())
    }

    async fn document(
        &self,
        client: &dyn ReadClient,
        scope: &Scope,
        company: OrganisationId,
        order: &PrintOrder<'_>,
        plan: &Traveller,
        accent: bilibi_ui::Accent,
    ) -> Result<bilibi_artifacts::InlineDocument> {
        let language = self
            .settings
            .get(
                client,
                scope,
                Target::Company(company),
                bilibi_templates::LANGUAGE,
            )
            .await?;
        let requested = language.value.as_str().unwrap_or("en");
        let selected = self
            .templates
            .resolve_print(client, scope, KEY, Some(company), requested, None)
            .await?;
        let layout = CONTRACT
            .compile(&selected.language, &selected.layout)
            .map_err(|_| Error::Invalid)?;
        let document = inline(&layout, order, plan, &selected.language, accent);
        match document {
            Err(problem)
                if problem.field == "layout"
                    && !matches!(selected.origin, Origin::Packaged { .. }) =>
            {
                let packaged = CONTRACT
                    .packaged(requested, None)
                    .map_err(|_| Error::Invalid)?;
                let layout = CONTRACT
                    .compile(&packaged.language, &packaged.layout)
                    .map_err(|_| Error::Invalid)?;
                inline(&layout, order, plan, &packaged.language, accent).map_err(|_| Error::Invalid)
            }
            other => other.map_err(|_| Error::Invalid),
        }
    }
}

fn inline(
    layout: &CompiledPrint<'_>,
    order: &PrintOrder<'_>,
    plan: &Traveller,
    language: &str,
    accent: bilibi_ui::Accent,
) -> std::result::Result<bilibi_artifacts::InlineDocument, Problem> {
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
            PrintValue::Date(due.parse().map_err(|_| Problem {
                field: "due_on".into(),
                message: "Invalid due date".into(),
            })?),
        ));
    }
    if let Some(demand) = order.demand_ref.filter(|text| !text.trim().is_empty()) {
        facts.push(("demand", PrintValue::Text(demand)));
    }
    layout.document(order.kind.name(), &facts, accent)
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
            let client = self
                .production
                .read_client(scope, company.get())
                .await
                .map_err(download_error)?;
            let (order, plan) = orders::get_traveller(&**client, scope, company.get(), id)
                .await
                .map_err(download_error)?;
            let accent = self.branding.accent(&**client).await?;
            let header = PrintOrder {
                code: &order.code,
                kind: order.kind,
                product_id: order.product_id,
                formula_version: order.formula_version,
                routing_version: order.routing_version,
                demand_ref: order.demand_ref.as_deref(),
            };
            let document = self
                .production
                .traveller_layouts
                .document(&**client, scope, company, &header, &plan, accent)
                .await?;
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

fn download_error(error: crate::Error) -> Error {
    match error {
        crate::Error::Access(error) => error,
        crate::Error::OrderNotFound => Error::NotFound,
        _ => Error::Internal,
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
