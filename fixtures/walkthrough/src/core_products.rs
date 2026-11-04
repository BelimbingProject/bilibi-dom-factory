//! Fixture setup only: a Core product for Factory to define. Core's own
//! walkthrough exercises its facade; Factory needs just a row to reference, so
//! this inserts one as the disposable database's owner.
use tokio_postgres::GenericClient;

/// A tenant's Core product of the given kind (`goods` or `service`), active.
pub async fn insert(
    client: &(impl GenericClient + Sync),
    tenant: i64,
    code: &str,
    name: &str,
    kind: &str,
) -> Result<i64, tokio_postgres::Error> {
    Ok(client
        .query_one(
            "INSERT INTO core_products(tenant_id,code,name,kind,base_unit) VALUES($1,$2,$3,$4,'each') RETURNING id",
            &[&tenant, &code, &name, &kind],
        )
        .await?
        .get(0))
}

/// Goods, the only kind a company can make.
pub async fn goods(
    client: &(impl GenericClient + Sync),
    tenant: i64,
    code: &str,
    name: &str,
) -> Result<i64, tokio_postgres::Error> {
    insert(client, tenant, code, name, "goods").await
}
