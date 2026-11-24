use bilibi_agent_api::{Handler, Registry};
use bilibi_database::{Error, MutationBoundary};
use bilibi_factory_product_definition::ProductDefinition;
use bilibi_factory_production_execution::{
    NewOrder, Order, ProductionExecution, WorkInstruction, WorkPlan,
};
use bilibi_settings::{Settings, Target};
use bilibi_tenancy::Scope;
use bilibi_ui::download::{DownloadRequest, DownloadService};
use serde_json::json;
use std::{collections::HashMap, path::Path, process::Command};

pub(super) async fn run(
    app: &bilibi_web::App,
    admin: &Scope,
    manager: &Scope,
    viewer: &Scope,
    outsider: &Scope,
    order: &Order,
    folder: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
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
    settings
        .put(
            &mut tx,
            admin,
            Target::Global,
            "pdf.chromium.binary",
            json!(std::env::var("FACTORY_TRAVELLER_CHROMIUM")?),
        )
        .await?;
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
    if !matches!(
        download.0.serve(request(outsider)).await,
        Err(Error::Forbidden | Error::NotFound)
    ) {
        return Err("the original traveller did not refuse the other tenant".into());
    }
    let original = download.0.serve(request(viewer)).await?;
    let original_path = folder.join("work-traveller-en.pdf");
    std::fs::write(&original_path, &original.bytes)?;
    let original_text = check_pdf(
        &original_path,
        "Work order traveller",
        "Routing version",
        "Sequence",
        (595.0, 842.0),
    )?;

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
    if !matches!(
        download.0.serve(request(outsider)).await,
        Err(Error::Forbidden | Error::NotFound)
    ) {
        return Err("the reprint did not refuse the other tenant".into());
    }
    let reprint_path = folder.join("work-traveller-reprint.pdf");
    std::fs::write(&reprint_path, &reprint.bytes)?;
    let reprint_text = check_pdf(
        &reprint_path,
        "Work order traveller",
        "Routing version",
        "Sequence",
        (595.0, 842.0),
    )?;
    if original_text != reprint_text {
        return Err("the English traveller changed after routing publication".into());
    }
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
    let malay_path = folder.join("work-traveller-ms.pdf");
    std::fs::write(&malay_path, &malay.bytes)?;
    check_pdf(
        &malay_path,
        "Kad kerja pengeluaran",
        "Versi laluan",
        "Turutan",
        (612.0, 792.0),
    )?;
    let codes: Vec<_> = (1..=66)
        .map(|sequence| format!("step-{sequence}"))
        .collect();
    let step = earlier
        .operations
        .first()
        .ok_or("missing routing operations")?;
    let large_routing: Vec<_> = codes
        .iter()
        .enumerate()
        .map(|(index, code)| {
            json!({
                "code": code,
                "sequence": index + 1,
                "inputs": step.inputs,
                "outputs": step.outputs,
                "allowed_resource_ids": step.allowed_resource_ids,
            })
        })
        .collect();
    let large_routing = definitions
        .publish_routing(
            manager,
            1,
            order.product_id,
            &json!({"operations":large_routing}),
        )
        .await?;

    let current = registry
        .read(
            &app.db,
            admin,
            "base.templates.print.get",
            &json!({"key":key,"company_id":1,"language":"ms"}),
        )
        .await?;
    let paragraph = json!({"type":"paragraph","text":"'".repeat(2_000)});
    layout["blocks"]
        .as_array_mut()
        .ok_or("missing traveller blocks")?
        .extend(std::iter::repeat_n(paragraph, 23));
    let input = json!({
        "key":key, "company_id":1, "language":"ms", "layout":layout.to_string(),
        "review":current["result"]["print"]["review"],
    });
    let mut client = app.db.get().await?;
    let mut tx = boundary
        .begin(&mut client, admin.mutation_context(operation))
        .await?;
    publish.preview(tx.sql(), admin, &input).await?;
    publish.commit(&mut tx, admin, &input).await?;
    tx.commit().await?;
    drop(client);

    let production = app
        .services
        .get::<ProductionExecution>(bilibi_factory_production_execution::SERVICE)?;
    let large_order = production
        .create_order(
            manager,
            1,
            &NewOrder {
                code: "ORD-LAYOUT-BUDGET".into(),
                kind: order.kind,
                product_id: order.product_id,
                formula_version: order.formula_version,
                routing_version: large_routing.version,
                demand_ref: None,
                work_plan: WorkPlan {
                    instructions: codes
                        .into_iter()
                        .map(|operation| WorkInstruction {
                            operation,
                            instructions: "'".repeat(10_000),
                        })
                        .collect(),
                    ..WorkPlan::default()
                },
            },
        )
        .await?;
    let large_path = HashMap::from([("id".into(), large_order.id.to_string())]);
    let large = download
        .0
        .serve(DownloadRequest {
            path: &large_path,
            ..request(viewer)
        })
        .await?;
    let fallback_path = folder.join("work-traveller-fallback.pdf");
    std::fs::write(&fallback_path, &large.bytes)?;
    check_paper(&fallback_path, (595.0, 842.0))?;
    println!("Travellers printed from order {} before and after the newer routing; the other tenant was refused.", order.id);
    Ok(())
}

fn check_pdf(
    path: &Path,
    heading: &str,
    routing_label: &str,
    sequence_label: &str,
    paper: (f64, f64),
) -> Result<String, Box<dyn std::error::Error>> {
    let text = poppler("pdftotext", &["-layout"], path, &["-"])?;
    let steps: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            Some((words.next()?.parse::<u32>().ok()?, words.next()?))
        })
        .collect();
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let routing = format!("{routing_label} 1");
    let expected_instruction = format!(
        "{}Final checkpoint: verify the final material identification.",
        "Record the inspection result and compare it with the assembly sheet. ".repeat(35)
    );
    let without_table_headers = text
        .lines()
        .filter(|line| line.split_whitespace().next() != Some(sequence_label))
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    let blend_instruction = without_table_headers
        .split_once("1 blend ")
        .and_then(|(_, rest)| rest.split_once(" 2 make"))
        .map(|(instruction, _)| instruction);
    if steps != [(1, "blend"), (2, "make"), (3, "fails")]
        || !normalized.contains(heading)
        || !normalized.contains(sequence_label)
        || !normalized.contains(&routing)
        || blend_instruction != Some(expected_instruction.as_str())
    {
        return Err(format!("{} has unexpected traveller text", path.display()).into());
    }

    check_paper(path, paper)?;
    Ok(text)
}

fn check_paper(path: &Path, paper: (f64, f64)) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = poppler("pdfinfo", &[], path, &[])?;
    let size = metadata
        .lines()
        .find_map(|line| line.strip_prefix("Page size:"))
        .ok_or("PDF has no page size")?;
    let mut size = size.split_whitespace();
    let width: f64 = size.next().ok_or("PDF has no page width")?.parse()?;
    if size.next() != Some("x") {
        return Err("PDF has an unexpected page size format".into());
    }
    let height: f64 = size.next().ok_or("PDF has no page height")?.parse()?;
    if (width - paper.0).abs() > 1.0 || (height - paper.1).abs() > 1.0 {
        return Err(format!("{} has unexpected paper dimensions", path.display()).into());
    }
    Ok(())
}

fn poppler(
    tool: &str,
    before: &[&str],
    path: &Path,
    after: &[&str],
) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new(tool)
        .env("LC_ALL", "C")
        .args(before)
        .arg(path)
        .args(after)
        .output()?;
    if !output.status.success() {
        return Err(format!("{tool} failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}
