# Product Definition

`ProductDefinition` owns resources, a company's manufacturing definition of a Core
product, and immutable formula/routing revisions. Resolve `SERVICE` (`factory/product_definition.product_definition`)
from the service registry. XD1 is the facade below; XD2 adds the pages.
It mirrors Bilimbi `ProductDefinition` at
`f39ed3dcf2ce2694d30c2b88e82212780f4f3b98`, except that products are Core's
([ADR 22](https://github.com/BelimbingProject/bilibi/blob/main/docs/architecture/decisions/22-shared-products-in-core.md)).

## One product list

A product a company makes and sells is a Core product (`core/product`): Core
holds its code, name, goods or service, base unit and selling prices, and has the
only product list. This module keeps only manufacturing facts, a **definition**
linked by the Core product's ID: which Inventory item the product yields, and its
formula and routing revisions. Never add a code, name or unit here; read them from
Core's `Products`. The ID in `product_id` everywhere in Factory (revisions,
production orders, agent operations) is that Core product ID.

Inventory items stay Factory's: an item is a stocked thing, including raw materials
and internal parts that are never sold, so it fails Core's "every seller needs it"
test. A finished item becomes a Core product only by a definition that names it.

## Facade

Every read takes a sealed scope and company, requires
`factory.product-definition.configuration.view`, and checks the live company.
Every write rechecks `.manage` and the authorized writable company inside
`MutationBoundary`. Both capabilities belong to `tenant_owner`.

- `create_definition`, `get_definition`, `list_definitions`: one definition per
  Core product and company, and one per Inventory item. The product must exist in
  Core, be goods (`kind`) and be active ("Product not found", "Product must be
  goods", "Product is retired"); item existence is checked through Inventory.
  There is no update or retirement: the Core product's retirement stops new
  definitions, while earlier ones stay readable.
- `create/update/retire/get/list_resource_type(s)`: uppercase trimmed code and
  company-defined properties using Inventory's shared property mechanism.
  Definitions freeze after any resource uses them, including retired resources.
  Retirement requires no active resources.
- `create/update/retire/get/list_resource(s)`: a fixed resource type and typed
  properties. Updates replace supplied properties; omission preserves them.
  Retired resources and types cannot be edited or retired again.
- `publish_formula`: JSON `{lines: [...], process_config: {...}}`. Each line has
  positive `item_id`, `unit_id`, `quantity`, and `role` (`input`/`output`). Optional
  `output_role` is a nonempty output-only string; `material_hold_rule` is an
  input-only object. There must be an output of the definition's item.
- `publish_routing`: JSON `{operations: [...], process_config: {...}}`. Each
  operation has unique nonempty `code`, unique positive `sequence`, `inputs`
  (possibly empty), nonempty `outputs` and `allowed_resource_ids`. ID lists hold
  positive integers. Input order is retained, not sorted by sequence.
- `get/list_formula_revisions`, `get/list_routing_revisions`: exact version reads
  and ascending version lists. Lists accept an optional cap (100 default, 500
  maximum), following Inventory's bounded-read convention rather than Bilimbi's
  unbounded lists. All lists include retired records where applicable.
- `select_revisions`: exact formula/routing versions, independently numbered.
  Routing inputs/outputs must be subsets of their respective formula item sets,
  and some operation must output the definition's item. It returns all three records.

`process_config` accepts only `process_family` (string/null), `tolerances`,
`output_roles` and `material_hold_rules` (object/null). Nested values remain
company data. Quantities become decimal strings. Resource properties use the
same definitions, normalization and validation as material properties.

## Pages

Two live pages, mirroring Bilimbi `ProductDefinition.Web.*Live`, each a
`FlexTable` with the built-in column settings, drag reorder and compact rows
(Bilimbi's lists have no sort, filter or pager, so these have none), and Factory's
panel on the Core product page. The route guard needs
`factory.product-definition.configuration.view` on every request and live event;
every write-shaped event asks for `.manage` again and the facade asks once more. A
view-only account sees the lists without controls. After a management grant is
revoked, a refused write keeps its permission message visible while the panel
reloads.

| Route | Page | Writes (flash) |
| --- | --- | --- |
| `/factory/resource-types` | Resource types (no menu item; the Resources page links to it) | New, Edit (code, name, property definitions as JSON), Retire behind a confirmation. "Resource type saved." / "Resource type retired." |
| `/factory/resources` | Resources (menu: Factory → Resources) | New, Edit (type fixed), Retire behind a confirmation. "Resource saved." / "Resource retired." |
| `/products/{id}` | **Factory** panel on the Core product page (no Factory menu item or list) | For a product with no definition, choose the output item and create it; then publish a formula or routing version from JSON. "Product definition created." / "Formula revision published." / "Routing revision published." |

The panel is a Module Registry `Embed` in slot `record.product` for
`EmbedContext::Record { kind: "core/product" }`. The host passes the product's
ID as `product_id`, shows it only to an account holding the view capability and
rechecks access on every event.

Differences from Bilimbi, all deliberate: retire asks through the Platform's
`ConfirmDialog` rather than `data-confirm`; the inline error is the Platform's
alert; a form's JSON that does not decode is shown as "Property definitions is
invalid" / "Properties is invalid" (Bilimbi hands the raw text to a changeset),
and for revisions it reaches the facade as text so it answers "Invalid lines" /
"Invalid operations" as in Bilimbi. The output item choices come from Inventory's
`list_items` (at most 500, as Bilimbi). An account with no company sees
"Company not found" (Bilimbi's nil `company_id`).

## Inventory and history

Publication checks materials and units through Inventory on the **same
transaction** and resources through this module. Retired references refuse new
publications. Bilimbi deliberately accepts `MaterialNotFound` (an item need not
be stocked); it does not check conversions or unit compatibility. Selection
does not revalidate retirement: historical revisions remain selectable.

Inventory reads retain their own capability checks: creating a definition also
needs Core's `products.view` and `commerce.inventory.item.view`; publication needs
`factory.inventory.configuration.view`. The company write lock serializes
publication with Inventory retirement and company archival. A product lock and
unique constraint serialize revision numbering. Failed writes consume no version.

All five tables have tenant/company ownership. Sibling Inventory IDs and the Core product ID are opaque
facade-validated references, not SQL dependencies on their tables. There is no
cross-container dependency. Revisions have no edit/delete API, and PostgreSQL
also refuses UPDATE, DELETE and TRUNCATE (stronger than the reference's
facade-only immutability). Every saved row is audited with complete old/new data.
There are no production seeds, agent operations, CSS or JavaScript.

The pages are private (`src/web`); the crate exports only the facade and its
service ID. Page proof: the container README's `product-definition-pages`.

## Runtime proof

Run the mounted Platform `scripts/domain-ci.sh` on an empty PostgreSQL 18
database. Then on a separate freshly migrated disposable database run the
container walkthrough's `product-definition` binary (root README). It uses
real sessions and the registered facades, including concurrent publication,
capability and company refusals, retirement, exact selection and SQL immutability.

## Production import reads

`select_import_revisions_on` and `get_import_resource_on` serve import validation on the caller's connection. They require Inventory's sealed `ProductionImport`, issued only for a registered posting authority, and recheck its module's import grant and live company. Ordinary Product Definition reads and every configuration write retain their existing capabilities. See [`Production Execution imports`](../production_execution/README.md#imports) for the Extension-facing API.
