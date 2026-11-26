//! Import-only read proof, called after the production execution walkthrough.
use super::{decimal, refusal, verify, Error};
use bilibi_authz::grants::Grants;
use bilibi_factory_inventory::{Inventory, NewUnit};
use bilibi_factory_production_execution::{ProductionExecution, ProductionTrace, Source};
use bilibi_tenancy::Scope;
use std::time::Duration;
use tokio::time::timeout;

pub(super) async fn run(
    app: &bilibi_web::App,
    admin: &Scope,
    manager: &Scope,
    principal: &Scope,
    evidence: (i64, i64, i64, [i64; 2]),
    resource_id: i64,
) -> Result<(), Error> {
    println!("== import-only material, conversion, yield and backward trace reads");
    let (material, execution, identity, inputs) = evidence;
    let inventory = app
        .services
        .get::<Inventory>(bilibi_factory_inventory::SERVICE)?;
    let pe = app
        .services
        .get::<ProductionExecution>(bilibi_factory_production_execution::SERVICE)?;
    let grants = app.services.get::<Grants>("base/authz.grants")?;
    grants
        .set_system(
            admin,
            "factory-import-fixture",
            "factory.production-execution.import",
            Some(true),
        )
        .await?;
    let importer = pe.importer(principal, 1)?;
    let unit = inventory
        .create_unit(
            manager,
            1,
            &NewUnit {
                code: "IMPORT-READ".into(),
                name: "Import read unit".into(),
            },
        )
        .await?;
    let first = inventory
        .define_conversion(manager, 1, material, unit.id, decimal("1.234567890123"))
        .await?;
    let second = inventory
        .define_conversion(manager, 1, material, unit.id, decimal("2.345678901234"))
        .await?;
    verify(
        "import material matches ordinary material",
        importer.get_material(material).await?
            == inventory.get_material(manager, 1, material).await?,
    )?;
    verify(
        "import conversion preserves exact old version after a new version",
        importer
            .get_conversion(material, unit.id, first.version)
            .await?
            == first
            && importer
                .get_conversion(material, unit.id, second.version)
                .await?
                == second,
    )?;
    refusal(
        "missing conversion version never falls back to current",
        importer
            .get_conversion(material, unit.id, second.version + 1)
            .await,
        "Conversion not found",
    )?;
    verify(
        "import yield matches ordinary native totals and conversion evidence",
        format!("{:?}", importer.get_run_yield(execution).await?)
            == format!("{:?}", pe.get_run_yield(manager, 1, execution).await?),
    )?;
    // Exercise the import-only yield reads of wastage and its correction chain.
    for capability in [
        "factory.production-execution.wastage.record",
        "factory.production-execution.wastage.correct",
    ] {
        grants
            .set_direct(
                admin,
                manager.user(),
                capability,
                Some(bilibi_authz::grants::Direct::Allow(
                    bilibi_authz::Reach::All,
                )),
            )
            .await?;
    }
    let reason = pe
        .create_wastage_reason(
            manager,
            1,
            &serde_json::json!({"code":"IMPORT-READ", "label":"Import read proof"}),
        )
        .await?;
    let posting = importer
        .get_transaction(
            importer
                .get_execution(execution)
                .await?
                .inventory_transaction_id,
        )
        .await?;
    let output = posting
        .entries
        .iter()
        .find(|entry| entry.identity_id == Some(identity) && entry.role == "stock")
        .ok_or("missing import output")?;
    let waste = pe.record_wastage(manager, 1, execution, &serde_json::json!({"request_id":"import-read-wastage", "reason_id":reason["id"], "item_id":material, "location_id":output.location_id, "identity_id":identity, "quantity":"0.25", "observation":"measured"})).await?;
    pe.correct_wastage(manager, 1, waste["id"].as_i64().ok_or("missing wastage id")?, &serde_json::json!({"request_id":"import-read-wastage-correction", "quantity":"0.125", "correction_reason":"Import read proof"})).await?;
    let corrected = importer.get_run_yield(execution).await?;
    verify(
        "import yield includes exact wastage and its correction postings",
        corrected.wastage_transaction_ids.len() == 2
            && corrected.balances.iter().any(|balance| {
                balance.wastage.parse::<bilibi_factory_inventory::Decimal>() == Ok(decimal("0.125"))
            }),
    )?;
    verify(
        "corrected import yield matches ordinary yield",
        format!("{corrected:?}") == format!("{:?}", pe.get_run_yield(manager, 1, execution).await?),
    )?;
    for capability in [
        "factory.production-execution.wastage.record",
        "factory.production-execution.wastage.correct",
    ] {
        grants
            .set_direct(admin, manager.user(), capability, None)
            .await?;
    }
    let verify_trace = |label, trace: &ProductionTrace| {
        verify(
            label,
            trace.material.root.id == identity
                && trace.runs.iter().any(|run| {
                    run.id == execution
                        && run.source == Source::Import
                        && run.resource.id == resource_id
                })
                && inputs.iter().all(|input| {
                    trace.material.identities.iter().any(|lot| lot.id == *input)
                        && trace.material.links.iter().any(|link| {
                            link.input_identity_id == *input && link.output_identity_id == identity
                        })
                }),
        )
    };
    let trace = importer.trace_backward(identity).await?;
    verify_trace("import trace includes both source lots and the run", &trace)?;
    let ordinary_trace = pe.trace_backward(manager, 1, identity).await?;
    verify_trace(
        "ordinary trace includes both source lots and the run",
        &ordinary_trace,
    )?;
    verify(
        "import backward trace matches ordinary genealogy and resources",
        format!("{trace:?}") == format!("{ordinary_trace:?}"),
    )?;

    // Exercise the two clients held by reviewed import: a preview transaction and
    // a mutation transaction. Nested reads must never acquire another pool slot.
    let expected_material = inventory.get_material(manager, 1, material).await?;
    let deadline = Duration::from_secs(10);
    timeout(deadline, async {
        let mut client = app.db.get().await?;
        let preview = client.transaction().await?;
        let preview_client = &*preview;
        verify(
            "preview import material uses held connection",
            importer.get_material_on(preview_client, material).await? == expected_material,
        )?;
        verify(
            "preview import keeps exact conversion",
            importer
                .get_conversion_on(preview_client, material, unit.id, first.version)
                .await?
                == first,
        )?;
        refusal(
            "preview missing version stays missing",
            importer
                .get_conversion_on(preview_client, material, unit.id, second.version + 1)
                .await,
            "Conversion not found",
        )?;
        verify_trace(
            "preview import trace includes both source lots and the run",
            &importer.trace_backward_on(preview_client, identity).await?,
        )?;
        verify(
            "preview ordinary reads use held connection",
            inventory
                .get_material_on(preview_client, manager, 1, material)
                .await?
                == expected_material
                && inventory
                    .get_conversion_on(
                        preview_client,
                        manager,
                        1,
                        material,
                        unit.id,
                        Some(first.version),
                    )
                    .await?
                    == first,
        )?;
        verify_trace(
            "preview ordinary trace includes both source lots and the run",
            &pe.trace_backward_on(preview_client, manager, 1, identity)
                .await?,
        )?;
        refusal(
            "preview ordinary trace denies import-only principal",
            pe.trace_backward_on(preview_client, principal, 1, identity)
                .await,
            "Access denied",
        )?;
        preview.rollback().await?;
        let boundary = app
            .services
            .get::<bilibi_database::MutationBoundary>("base/database.mutations")?;
        let tx = boundary
            .begin(
                &mut client,
                principal.mutation_context("walkthrough.import-revalidation"),
            )
            .await?;
        verify(
            "revalidation import material uses mutation connection",
            importer.get_material_on(tx.sql(), material).await? == expected_material,
        )?;
        verify(
            "revalidation keeps exact conversion",
            importer
                .get_conversion_on(tx.sql(), material, unit.id, first.version)
                .await?
                == first,
        )?;
        verify_trace(
            "revalidation trace includes both source lots and the run",
            &importer.trace_backward_on(tx.sql(), identity).await?,
        )?;
        Ok::<_, Error>(())
    })
    .await
    .map_err(|_| "preview or revalidation exceeded the held-connection deadline")??;

    refusal(
        "ordinary material read retains configuration view",
        inventory.get_material(principal, 1, material).await,
        "Access denied",
    )?;
    refusal(
        "ordinary exact conversion retains configuration view",
        inventory
            .get_conversion(principal, 1, material, unit.id, Some(first.version))
            .await,
        "Access denied",
    )?;
    refusal(
        "ordinary run yield retains floor view",
        pe.get_run_yield(principal, 1, execution).await,
        "Access denied",
    )?;
    refusal(
        "ordinary backward trace retains floor view",
        pe.trace_backward(principal, 1, identity).await,
        "Access denied",
    )?;
    refusal(
        "ordinary Inventory trace retains configuration view",
        inventory.trace_backward(principal, 1, identity).await,
        "Access denied",
    )?;

    let db = app
        .services
        .get::<bilibi_database::Db>("base/database.pool")?;
    let foreign_importer = timeout(deadline, async {
        let client = app.db.get().await?;
        let foreign: i64 = client
            .query_one("SELECT id FROM companies WHERE tenant_id<>1 LIMIT 1", &[])
            .await?
            .get(0);
        let foreign_importer = pe.importer(principal, foreign)?;
        refusal(
            "held connection refuses foreign material",
            foreign_importer.get_material_on(&**client, material).await,
            "Not found",
        )?;
        refusal(
            "held connection refuses foreign conversion",
            foreign_importer
                .get_conversion_on(&**client, material, unit.id, first.version)
                .await,
            "Not found",
        )?;
        refusal(
            "held connection refuses foreign trace",
            foreign_importer
                .trace_backward_on(&**client, identity)
                .await,
            "Not found",
        )?;
        Ok::<_, Error>(foreign_importer)
    })
    .await
    .map_err(|_| "foreign-company checks exceeded the held-connection deadline")??;
    refusal(
        "material import read refuses a foreign company",
        foreign_importer.get_material(material).await,
        "Not found",
    )?;
    refusal(
        "conversion import read refuses a foreign company",
        foreign_importer
            .get_conversion(material, unit.id, first.version)
            .await,
        "Not found",
    )?;
    refusal(
        "yield import read refuses a foreign company",
        foreign_importer.get_run_yield(execution).await,
        "Not found",
    )?;
    refusal(
        "trace import read refuses a foreign company",
        foreign_importer.trace_backward(identity).await,
        "Not found",
    )?;
    refusal(
        "missing material stays missing",
        importer.get_material(i64::MAX).await,
        "Item not found",
    )?;
    refusal(
        "missing run stays missing",
        importer.get_run_yield(i64::MAX).await,
        "Execution not found",
    )?;
    refusal(
        "missing lot stays missing",
        importer.trace_backward(i64::MAX).await,
        "Identity not found",
    )?;

    let client = db.get().await?;
    client
        .execute(
            "UPDATE companies SET status='archived' WHERE tenant_id=1 AND id=1",
            &[],
        )
        .await?;
    drop(client);
    verify(
        "archived company preserves import material evidence",
        importer.get_material(material).await?.item_id == material,
    )?;
    verify(
        "archived company preserves exact import conversion evidence",
        importer
            .get_conversion(material, unit.id, first.version)
            .await?
            == first,
    )?;
    verify(
        "archived company preserves import run yield",
        importer.get_run_yield(execution).await?.execution_id == execution,
    )?;
    verify_trace(
        "archived company trace includes both source lots and the run",
        &importer.trace_backward(identity).await?,
    )?;
    let client = db.get().await?;
    client
        .execute(
            "UPDATE companies SET deleted_at=now() WHERE tenant_id=1 AND id=1",
            &[],
        )
        .await?;
    drop(client);
    refusal(
        "material import read rechecks live company",
        importer.get_material(material).await,
        "Not found",
    )?;
    refusal(
        "conversion import read rechecks live company",
        importer
            .get_conversion(material, unit.id, first.version)
            .await,
        "Not found",
    )?;
    refusal(
        "yield import read rechecks live company",
        importer.get_run_yield(execution).await,
        "Not found",
    )?;
    refusal(
        "trace import read rechecks live company",
        importer.trace_backward(identity).await,
        "Not found",
    )?;
    let client = db.get().await?;
    client
        .execute(
            "UPDATE companies SET status='active', deleted_at=NULL WHERE tenant_id=1 AND id=1",
            &[],
        )
        .await?;
    drop(client);
    grants
        .set_system(
            admin,
            "factory-import-fixture",
            "factory.production-execution.import",
            Some(false),
        )
        .await?;
    timeout(deadline, async {
        let client = app.db.get().await?;
        refusal(
            "held connection rechecks revoked material grant",
            importer.get_material_on(&**client, material).await,
            "Access denied",
        )?;
        refusal(
            "held connection rechecks revoked conversion grant",
            importer
                .get_conversion_on(&**client, material, unit.id, first.version)
                .await,
            "Access denied",
        )?;
        refusal(
            "held connection rechecks revoked trace grant",
            importer.trace_backward_on(&**client, identity).await,
            "Access denied",
        )?;
        Ok::<_, Error>(())
    })
    .await
    .map_err(|_| "revoked-grant checks exceeded the held-connection deadline")??;
    refusal(
        "retained importer material read rechecks revoked grant",
        importer.get_material(material).await,
        "Access denied",
    )?;
    refusal(
        "retained importer conversion rechecks revoked grant",
        importer
            .get_conversion(material, unit.id, first.version)
            .await,
        "Access denied",
    )?;
    refusal(
        "retained importer yield rechecks revoked grant",
        importer.get_run_yield(execution).await,
        "Access denied",
    )?;
    refusal(
        "retained importer trace rechecks revoked grant",
        importer.trace_backward(identity).await,
        "Access denied",
    )?;
    Ok(())
}
