//! Private work order travels with its saved routing and the recorded plan.
use crate::{
    contributions::FLOOR_VIEW,
    traveller_layout::{CONTRACT, KEY},
    Order, OrderKind, ProductionExecution,
};
use bilibi_artifacts::{Artifacts, Operation, Owner, PdfOptions, Reference};
use bilibi_branding::Branding;
use bilibi_database::{Error, Result};
use bilibi_module_registry::{
    contributions::{Access, Download as Route, DOWNLOADS},
    Contribution, Provenance,
};
use bilibi_party_ids::OrganisationId;
use bilibi_settings::{Settings, Target};
use bilibi_templates::{CompiledPrint, Origin, PrintFacts, PrintValue, Templates};
use bilibi_tenancy::{ProofClient, ProofFuture, Scope};
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

impl Owner for TravellerDownload {
    fn owner_id(&self) -> &'static str {
        "factory/production_execution"
    }

    fn authorize<'a>(
        &'a self,
        client: &'a dyn ProofClient,
        scope: &'a Scope,
        company: OrganisationId,
        operation: Operation,
        reference: Option<&'a Reference>,
    ) -> ProofFuture<'a, ()> {
        Box::pin(async move {
            if !matches!(operation, Operation::Create | Operation::Read) {
                return Err(Error::Forbidden);
            }
            bilibi_company::authorize_work_target(client, scope, company.get(), FLOOR_VIEW).await?;
            let order = reference
                .filter(|reference| reference.kind == KIND)
                .and_then(|reference| reference.subject.parse::<i64>().ok())
                .filter(|id| *id > 0)
                .ok_or(Error::NotFound)?;
            client.query_opt(
                "SELECT id FROM factory_production_orders WHERE tenant_id=$1 AND company_id=$2 AND id=$3",
                &[&scope.tenant(), &company.get(), &order]
            ).await?.ok_or(Error::NotFound)?;
            Ok(())
        })
    }
}

fn inline(
    layout: &CompiledPrint<'_>,
    order: &Order,
    malay: bool,
    accent: bilibi_ui::Accent,
) -> Result<bilibi_artifacts::InlineDocument> {
    let plan = &order.traveller;
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
        ("number", PrintValue::Text(order.code.as_str())),
        ("kind", PrintValue::Text(kind)),
        ("issuer", PrintValue::Text(&plan.issuer)),
        ("product", PrintValue::Integer(plan.product)),
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
    if let Some(demand) = order
        .demand_ref
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
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
            let mut client = self.production.db.get().await?;
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
            let malay = selected.language.split('-').next() == Some("ms");
            let accent = self.branding.accent(&**client).await?;
            let document = inline(&layout, &order, malay, accent)?;
            let revision = match selected.origin {
                Origin::Packaged { version } => format!("packaged-{version}"),
                Origin::Tenant { revision } | Origin::Company { revision, .. } => {
                    format!("revision-{revision}")
                }
            };
            let options = PdfOptions::default()
                .template_version(format!("{}-{revision}", selected.language))
                .data_version(format!(
                    "{}-{}-{}",
                    CONTRACT.version, order.formula_version, order.routing_version
                ));
            let owner = self.artifacts.for_owner(scope, company, self);
            let stored = owner
                .render_inline_document(
                    &mut client,
                    Reference {
                        subject: id.to_string(),
                        kind: KIND.into(),
                    },
                    document,
                    &options,
                )
                .await
                .map_err(artifact_error)?;
            let pdf = owner
                .read(&mut client, &stored.document.id)
                .await
                .map_err(artifact_error)?;
            Ok(FileBody::inline(
                pdf.bytes,
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
