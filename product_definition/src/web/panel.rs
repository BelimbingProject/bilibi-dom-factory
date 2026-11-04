//! Factory's panel on the Core product page: the product's manufacturing
//! definition and its immutable formula and routing revisions (Bilimbi
//! `ProductDefinition.Web.DefinitionsLive`, scoped to one Core product).
use super::shared::{self as s, Notice, Result};
use super::Pages;
use crate::{contributions::MANAGE, Definition, FormulaRevision, NewDefinition, RoutingRevision};
use askama::Template;
use bilibi_factory_inventory::Item;
use bilibi_tenancy::Scope;
use bilibi_ui::{
    Alert, Card, Choice, EmptyState, FlexRow, Html, InputKind, ListColumns, RowMode, StatusKind,
};
use serde_json::{json, Value};

const DENIED: &str = "You do not have permission to manage definitions.";
const PRODUCT: &str = "product";
/// The root element the Core product page gives this embed: `product-panel-` and
/// the contribution ID (`factory/product_definition.product`) with `.` and `/` as `-`.
const PANEL: &str = "product-panel-factory-product_definition-product";
const FORMULA: &str = "formula";
const ROUTING: &str = "routing";

/// A revision's JSON, one block in a normal row and one line in a compact one.
#[derive(Template)]
#[template(
    source = r#"<pre class="whitespace-pre-wrap text-xs">{{ text }}</pre>"#,
    ext = "html"
)]
struct Block<'a> {
    text: &'a str,
}

#[derive(Clone, Default)]
struct Draft {
    values: [String; 2],
}

pub(super) struct ProductPanel {
    pages: Pages,
    product_id: i64,
    formulas_columns: ListColumns,
    routings_columns: ListColumns,
    definition: Option<Definition>,
    item: Option<Item>,
    items: Vec<Item>,
    formulas: Vec<FormulaRevision>,
    routings: Vec<RoutingRevision>,
    definition_draft: String,
    formula_draft: Draft,
    routing_draft: Draft,
    can_manage: bool,
    notice: Notice,
    saved: Option<&'static str>,
}

fn blank_publish() -> Draft {
    Draft {
        values: ["[]".into(), "{}".into()],
    }
}

impl ProductPanel {
    pub(super) fn new(pages: &Pages, product_id: i64) -> Result<Self> {
        let revisions = |id: &str, body: &str| {
            s::mount_columns(
                id,
                vec![
                    s::column("version", "Version"),
                    s::column("body", body),
                    s::column("process_config", "Process configuration"),
                ],
            )
        };
        Ok(Self {
            pages: pages.clone(),
            product_id,
            formulas_columns: revisions("formula-revisions-table", "Lines")?,
            routings_columns: revisions("routing-revisions-table", "Operations")?,
            definition: None,
            item: None,
            items: Vec::new(),
            formulas: Vec::new(),
            routings: Vec::new(),
            definition_draft: String::new(),
            formula_draft: blank_publish(),
            routing_draft: blank_publish(),
            can_manage: false,
            notice: Notice::default(),
            saved: None,
        })
    }

    async fn load(&mut self, scope: &Scope) -> Result<()> {
        self.can_manage = s::can(&self.pages.db, scope, MANAGE).await?;
        self.definition = None;
        self.item = None;
        self.items = Vec::new();
        self.formulas = Vec::new();
        self.routings = Vec::new();
        let Some(company) = s::company(scope) else {
            self.notice.fail("Company not found");
            return Ok(());
        };
        let facade = &self.pages.definitions;
        match facade.get_definition(scope, company, self.product_id).await {
            Ok(definition) => self.definition = Some(definition),
            // No definition yet is the ordinary state of a product Factory does not make.
            Err(crate::Error::DefinitionNotFound) => {}
            Err(error) => {
                if let Err(message) = s::refusal::<()>(Err(error))? {
                    self.notice.fail(message);
                }
                return Ok(());
            }
        }
        let Some(definition) = self.definition.clone() else {
            if self.can_manage {
                let items = self
                    .pages
                    .inventory
                    .list_items(scope, company, None, Some(500))
                    .await
                    .map_err(crate::Error::from);
                match s::refusal(items)? {
                    Ok(items) => self.items = items,
                    Err(message) => self.notice.fail(message),
                }
            }
            return Ok(());
        };
        let item = self
            .pages
            .inventory
            .get_item(scope, company, definition.item_id)
            .await
            .map_err(crate::Error::from);
        // The output item is a label for the person; its refusal never hides the revisions.
        self.item = s::refusal(item)?.ok();
        let formulas = facade
            .list_formula_revisions(scope, company, self.product_id, None)
            .await;
        let routings = facade
            .list_routing_revisions(scope, company, self.product_id, None)
            .await;
        match (s::refusal(formulas)?, s::refusal(routings)?) {
            (Ok(formulas), Ok(routings)) => {
                self.formulas = formulas;
                self.routings = routings;
                self.formulas_columns
                    .load(&self.formulas, |f| f.version.to_string());
                self.routings_columns
                    .load(&self.routings, |r| r.version.to_string());
            }
            (Err(message), _) | (_, Err(message)) => self.notice.fail(message),
        }
        Ok(())
    }

    pub(super) async fn refresh(&mut self, scope: &Scope) -> Result<()> {
        self.load(scope).await
    }

    pub(super) async fn event(&mut self, scope: &Scope, payload: &Value) -> Result<()> {
        self.notice.begin();
        self.notice.error = None;
        self.saved = None;
        let kind = s::text(payload, "type").ok_or(bilibi_database::Error::Invalid)?;
        match kind {
            "clear-flash" => return Ok(()),
            // Each table raises its own event, so its columns are the ones touched.
            "grid_formulas" => {
                s::grid(&mut self.formulas_columns, payload);
                return Ok(());
            }
            "grid_routings" => {
                s::grid(&mut self.routings_columns, payload);
                return Ok(());
            }
            "create_definition" | "publish_formula" | "publish_routing" => {}
            _ => return Err(bilibi_database::Error::Invalid),
        }
        if !s::can(&self.pages.db, scope, MANAGE).await? {
            self.load(scope).await?;
            self.notice.fail(DENIED);
            return Ok(());
        }
        match kind {
            "create_definition" => self.create_definition(scope, payload).await?,
            "publish_formula" => self.publish(scope, payload, true).await?,
            _ => self.publish(scope, payload, false).await?,
        }
        Ok(())
    }

    async fn create_definition(&mut self, scope: &Scope, payload: &Value) -> Result<()> {
        let posted = s::form(payload, PRODUCT);
        let draft = s::pick(&posted, "item_id");
        let Some(company) = s::company(scope) else {
            self.definition_draft = draft;
            self.notice.fail("Company not found");
            return Ok(());
        };
        let item_id = draft.trim().parse::<i64>().ok();
        let done = if let Some(item_id) = item_id {
            self.pages
                .definitions
                .create_definition(
                    scope,
                    company,
                    &NewDefinition {
                        product_id: self.product_id,
                        item_id,
                    },
                )
                .await
        } else {
            Err(crate::Error::Inventory(
                bilibi_factory_inventory::Error::ItemNotFound,
            ))
        };
        match s::refusal(done)? {
            Ok(_) => {
                self.definition_draft = String::new();
                self.load(scope).await?;
                self.saved = Some("Product definition created.");
            }
            Err(message) => {
                self.definition_draft = draft;
                self.load(scope).await?;
                self.notice.fail(message);
            }
        }
        Ok(())
    }

    async fn publish(&mut self, scope: &Scope, payload: &Value, formula: bool) -> Result<()> {
        let (form, first) = if formula {
            (FORMULA, "lines")
        } else {
            (ROUTING, "operations")
        };
        let posted = s::form(payload, form);
        let draft = Draft {
            values: [s::pick(&posted, first), s::pick(&posted, "process_config")],
        };
        let product = self.product_id;
        let Some(company) = s::company(scope) else {
            self.notice.fail("Company not found");
            return Ok(());
        };
        let attrs = json!({
            first: s::decode(&draft.values[0]),
            "process_config": s::decode(&draft.values[1]),
        });
        let facade = &self.pages.definitions;
        let done = if formula {
            facade
                .publish_formula(scope, company, product, &attrs)
                .await
                .map(drop)
        } else {
            facade
                .publish_routing(scope, company, product, &attrs)
                .await
                .map(drop)
        };
        match s::refusal(done)? {
            Ok(()) => {
                if formula {
                    self.formula_draft = blank_publish();
                } else {
                    self.routing_draft = blank_publish();
                }
                self.load(scope).await?;
                self.saved = Some(if formula {
                    "Formula revision published."
                } else {
                    "Routing revision published."
                });
            }
            Err(message) => {
                if formula {
                    self.formula_draft = draft;
                } else {
                    self.routing_draft = draft;
                }
                self.load(scope).await?;
                self.notice.fail(message);
            }
        }
        Ok(())
    }

    fn block(columns: &ListColumns, value: &Value) -> Result<Html> {
        let text = s::json(value, columns.mode() != RowMode::Compact);
        s::render(Block { text: &text })
    }

    /// A line as stored: absent optional keys stay absent, not `null`.
    fn lines(revision: &FormulaRevision) -> Value {
        let lines = revision
            .lines
            .iter()
            .map(|line| {
                let mut value = json!(line);
                if let Some(map) = value.as_object_mut() {
                    map.retain(|_, v| !v.is_null());
                }
                value
            })
            .collect();
        Value::Array(lines)
    }

    fn formula_rows(&self) -> Result<Vec<FlexRow>> {
        self.formulas
            .iter()
            .map(|formula| {
                let mut row = FlexRow::new(formula.version.to_string());
                row.dom_id = Some(format!("formula-revision-{}", formula.version));
                row = s::slot(row, "version", &formula.version.to_string());
                Ok(row
                    .slot(
                        "body",
                        Self::block(&self.formulas_columns, &Self::lines(formula))?,
                    )
                    .slot(
                        "process_config",
                        Self::block(
                            &self.formulas_columns,
                            &Value::Object(formula.process_config.clone()),
                        )?,
                    ))
            })
            .collect()
    }

    fn routing_rows(&self) -> Result<Vec<FlexRow>> {
        self.routings
            .iter()
            .map(|routing| {
                let mut row = FlexRow::new(routing.version.to_string());
                row.dom_id = Some(format!("routing-revision-{}", routing.version));
                row = s::slot(row, "version", &routing.version.to_string());
                Ok(row
                    .slot(
                        "body",
                        Self::block(&self.routings_columns, &json!(routing.operations))?,
                    )
                    .slot(
                        "process_config",
                        Self::block(
                            &self.routings_columns,
                            &Value::Object(routing.process_config.clone()),
                        )?,
                    ))
            })
            .collect()
    }

    /// The only write before a definition exists: which item the product yields.
    fn definition_form(&self) -> Result<Html> {
        let items = self
            .items
            .iter()
            .map(|item| {
                Choice::new(
                    format!("{} — {}", item.sku, item.title),
                    item.id.to_string(),
                )
            })
            .collect();
        let item = s::text_field(PRODUCT, "item_id", "Output item", &self.definition_draft)
            .kind(InputKind::Select)
            .prompt("Choose an item")
            .options(items);
        s::render(s::FormCard {
            form_id: "product-form",
            event: "create_definition",
            fields: vec![item],
            submit: "Create product definition",
            cancel: false,
        })
    }

    /// One revision card: its table, then the publish form when allowed.
    #[allow(clippy::too_many_arguments)]
    fn revisions_card(&self, formula: bool, rows: Vec<FlexRow>) -> Result<Html> {
        let (id, noun, columns, draft, form_id, event, label, hint, button) = if formula {
            (
                "formula-revisions",
                "formulas",
                &self.formulas_columns,
                &self.formula_draft,
                "formula-form",
                "publish_formula",
                "Lines (JSON)",
                "Each line names item_id, unit_id, role, and quantity.",
                "Publish formula version",
            )
        } else {
            (
                "routing-revisions",
                "routings",
                &self.routings_columns,
                &self.routing_draft,
                "routing-form",
                "publish_routing",
                "Operations (JSON)",
                "Each operation names code, sequence, inputs, outputs, and allowed_resource_ids.",
                "Publish routing version",
            )
        };
        let (empty, reason, caption) = if formula {
            (
                "No formulas",
                "Publish a formula revision to define inputs and outputs.",
                "Formula revisions",
            )
        } else {
            (
                "No routings",
                "Publish a routing revision to define operations.",
                "Routing revisions",
            )
        };
        let namespace = if formula { FORMULA } else { ROUTING };
        let first = if formula { "lines" } else { "operations" };
        let mut body = s::table_in(
            PANEL,
            &format!("{id}-table"),
            if formula {
                "grid_formulas"
            } else {
                "grid_routings"
            },
            caption,
            columns,
            rows,
            false,
            (empty, reason),
        )?;
        if self.can_manage {
            body.push(&s::render(s::FormCard {
                form_id,
                event,
                fields: vec![
                    s::textarea_field(namespace, first, label, hint, &draft.values[0]),
                    s::textarea_field(
                        namespace,
                        "process_config",
                        "Process configuration (JSON)",
                        "",
                        &draft.values[1],
                    ),
                ],
                submit: button,
                cancel: false,
            })?);
        }
        s::render(Card {
            id,
            title: &format!("Product {noun}"),
            body,
            ..Card::default()
        })
    }

    pub(super) fn view(&self) -> Result<String> {
        let mut body = self.notice.alert("definition-error")?;
        if let Some(message) = self.saved {
            body.push(&s::render(Alert {
                kind: StatusKind::Success,
                id: "definition-saved",
                class: "mt-4",
                body: Html::text(message),
            })?);
        }
        body.push(&self.definition_card()?);
        if self.definition.is_some() {
            let mut cards = self.revisions_card(true, self.formula_rows()?)?;
            cards.push(&self.revisions_card(false, self.routing_rows()?)?);
            body.push(&s::render(Revisions { body: cards })?);
        }
        Ok(s::render(Panel { id: PANEL, body })?.as_str().to_owned())
    }

    /// What Factory knows about the product: the item it yields, or how to define one.
    fn definition_card(&self) -> Result<Html> {
        let body = if let Some(definition) = &self.definition {
            let label = match &self.item {
                Some(item) => format!("{} — {}", item.sku, item.title),
                None => format!("Item {}", definition.item_id),
            };
            s::render(Facts {
                label: "Output item",
                value: &label,
            })?
        } else if self.can_manage {
            self.definition_form()?
        } else {
            s::render(EmptyState::missing(
                "No product definition",
                "Factory has not defined how this product is made.",
            ))?
        };
        s::render(Card {
            id: "product-definition",
            title: "Factory",
            body,
            ..Card::default()
        })
    }
}

/// The panel's frame: the host supplies the page and its header. Its ID is the
/// one the Core product page gives this embed (`product-panel-` and the
/// contribution ID with `.` and `/` as `-`); the host sends the panel's events
/// to the element it names.
#[derive(Template)]
#[template(
    source = r#"<div id="{{ id }}" data-target="{{ id }}" class="space-y-5">{{ body|safe }}</div>"#,
    ext = "html"
)]
struct Panel<'a> {
    id: &'a str,
    body: Html,
}

#[derive(Template)]
#[template(
    source = r#"<dl class="p-4"><dt class="text-sm text-muted">{{ label }}</dt><dd id="product-definition-item">{{ value }}</dd></dl>"#,
    ext = "html"
)]
struct Facts<'a> {
    label: &'a str,
    value: &'a str,
}

/// The product's two revision cards, spaced as Bilimbi's `mt-5 space-y-5`.
#[derive(Template)]
#[template(
    source = r#"<div class="mt-5 space-y-5">{{ body|safe }}</div>"#,
    ext = "html"
)]
struct Revisions {
    body: Html,
}
