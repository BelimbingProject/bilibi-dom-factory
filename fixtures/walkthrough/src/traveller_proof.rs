use bilibi_agent_api::{Handler, Registry};
use bilibi_database::{Error, MutationBoundary};
use bilibi_factory_product_definition::ProductDefinition;
use bilibi_factory_production_execution::Order;
use bilibi_settings::{Settings, Target};
use bilibi_tenancy::Scope;
use bilibi_ui::download::{DownloadRequest, DownloadService};
use serde_json::json;
use std::{collections::HashMap, os::unix::fs::PermissionsExt, path::Path};

pub(super) async fn run(
    app: &bilibi_web::App,
    admin: &Scope,
    manager: &Scope,
    viewer: &Scope,
    outsider: &Scope,
    order: &Order,
    folder: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let storage = folder.join("private-artifacts");
    std::fs::create_dir_all(&storage)?;
    std::fs::set_permissions(&storage, std::fs::Permissions::from_mode(0o700))?;
    let settings = app.services.get::<Settings>("base/settings.store")?;
    let boundary = app
        .services
        .get::<MutationBoundary>("base/database.mutations")?;
    let mut client = app.db.get().await?;
    let mut tx = boundary
        .begin(
            &mut client,
            admin.mutation_context("walkthrough.work-traveller"),
        )
        .await?;
    for (key, value) in [
        ("artifacts.storage_root", json!(storage)),
        ("artifacts.retention_days", json!(30)),
        (
            "pdf.chromium.binary",
            json!(std::env::var("FACTORY_TRAVELLER_CHROMIUM")?),
        ),
    ] {
        settings
            .put(&mut tx, admin, Target::Global, key, value)
            .await?;
    }
    tx.commit().await?;
    drop(client);

    let download = app
        .services
        .get::<DownloadService<Scope, Error>>("factory/production_execution.traveller_download")?;
    let path = HashMap::from([("id".into(), order.id.to_string())]);
    let query = HashMap::new();
    let request = |scope| DownloadRequest {
        operation: "work-order-traveller",
        scope,
        path: &path,
        query: &query,
    };
    if download.0.serve(request(outsider)).await.is_ok() {
        return Err("another tenant received the work order".into());
    }
    let original = download.0.serve(request(viewer)).await?;
    if !original.bytes.starts_with(b"%PDF-") {
        return Err("work traveller was not a PDF".into());
    }
    std::fs::write(folder.join("work-traveller-en.pdf"), &original.bytes)?;

    let definitions = app
        .services
        .get::<ProductDefinition>(bilibi_factory_product_definition::SERVICE)?;
    let earlier = definitions
        .get_routing_revision(manager, 1, order.product_id, order.routing_version)
        .await?;
    let operations = earlier
        .operations
        .iter()
        .map(|step| {
            json!({
                "code": format!("revised-{}", step.code),
                "sequence": step.sequence,
                "inputs": step.inputs,
                "outputs": step.outputs,
                "allowed_resource_ids": step.allowed_resource_ids
            })
        })
        .collect::<Vec<_>>();
    definitions
        .publish_routing(
            manager,
            1,
            order.product_id,
            &json!({"operations":operations}),
        )
        .await?;
    let reprint = download.0.serve(request(viewer)).await?;
    std::fs::write(folder.join("work-traveller-reprint.pdf"), &reprint.bytes)?;
    let registry = app.services.get::<Registry>("base/agent_api.registry")?;
    let key = "factory.production_execution.traveller";
    let current = registry
        .read(
            &app.db,
            admin,
            "base.templates.print.get",
            &json!({"key":key,"company_id":1,"language":"ms"}),
        )
        .await?;
    let print = &current["result"]["print"];
    let mut layout: serde_json::Value = serde_json::from_str(
        print["effective"]["layout"]
            .as_str()
            .ok_or("missing traveller layout")?,
    )?;
    layout["paper"] = json!("letter");
    let input = json!({
        "key":key, "company_id":1, "language":"ms", "layout":layout.to_string(), "review":print["review"]
    });
    let operation = "base.templates.print.publish";
    let Handler::SafeWrite(publish) = registry.operations[operation].definition.handler else {
        return Err("print publication did not use a reviewed write".into());
    };
    let mut client = app.db.get().await?;
    let mut tx = boundary
        .begin(&mut client, admin.mutation_context(operation))
        .await?;
    publish.preview(tx.sql(), admin, &input).await?;
    publish.commit(&mut tx, admin, &input).await?;
    let company = viewer.current_company().ok_or("viewer has no company")?;
    settings
        .put(
            &mut tx,
            admin,
            Target::Company(company),
            "templates.language",
            json!("ms"),
        )
        .await?;
    tx.commit().await?;
    drop(client);
    let malay = download.0.serve(request(viewer)).await?;
    std::fs::write(folder.join("work-traveller-ms.pdf"), &malay.bytes)?;
    println!("Travellers printed from order {} before and after the newer routing; the other tenant was refused.", order.id);
    Ok(())
}
