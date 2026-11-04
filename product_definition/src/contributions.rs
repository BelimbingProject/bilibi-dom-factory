//! Bilimbi.Factory.ProductDefinition.Contributions: the vocabulary, the
//! Factory menu item, the two page routes and the Core product panel. Resource types has no menu
//! item in Bilimbi either: the Resources page links to it.
use bilibi_authz::{CapabilityDefinition as C, RoleDefinition, VocabularyContribution, VOCABULARY};
use bilibi_menu::{MenuItem, Requirement};
use bilibi_module_registry::contributions::{
    Access, Embed, EmbedContext, Method, Route, EMBEDS, ROUTES,
};
use bilibi_module_registry::{Contribution, Provenance};
pub(crate) const VIEW: &str = "factory.product-definition.configuration.view";
pub(crate) const MANAGE: &str = "factory.product-definition.configuration.manage";
inventory::submit! { Contribution::new(VOCABULARY,"factory/product_definition",Provenance {
    module:"factory/product_definition",source:file!(),line:line!(),
}, &VocabularyContribution {
    domains:&["factory"],verbs:&[],capabilities:&[C::tenant(VIEW),C::tenant(MANAGE)],
    roles:&[RoleDefinition {code:"tenant_owner",name:"Tenant Owner",description:"Contributed system role; reconciled explicitly",grant_all:false,capabilities:&[VIEW,MANAGE]}],
    system_principals:&[],
}) }

bilibi_menu::contribute!(
    "factory/product_definition",
    MenuItem::page("admin.factory.resources", "Resources", "/factory/resources")
        .company_scoped()
        .under("admin.factory")
        .requires(Requirement::Capability(VIEW))
        .order(60)
);
macro_rules! page_route {
    ($id:literal, $path:literal, $operation:literal) => {
        inventory::submit! { Contribution::new(ROUTES, $id, Provenance {
            module:"factory/product_definition",source:file!(),line:line!(),
        }, &Route { company_scope: bilibi_module_registry::contributions::CompanyScope::Company,
            method:Method::Get, path:$path, service:crate::PAGES, operation:$operation,
            access:Access::Capability(VIEW), framed:true,
        }) }
    };
}
page_route!(
    "factory/product_definition.resource-types",
    "/factory/resource-types",
    "resource-types"
);
page_route!(
    "factory/product_definition.resources",
    "/factory/resources",
    "resources"
);

// Factory's panel on the one Core product page: the product's manufacturing
// definition and its formula and routing revisions. There is no Factory
// product list or menu item, because Core's Products is the one list.
inventory::submit! { Contribution::new(EMBEDS,"factory/product_definition.product",Provenance {
    module:"factory/product_definition",source:file!(),line:line!(),
}, &Embed {
    slot:"record.product", context:EmbedContext::Record {kind:"core/product"},
    service:crate::PAGES, operation:"product-panel", access:Access::Capability(VIEW),
}) }

// ADR 01 gap, listed so it stays visible: these pages still write through
// the Product Definition facade rather than an operation contract.
inventory::submit! { Contribution::new(bilibi_module_registry::contributions::PAGE_WRITES,crate::PAGES,Provenance {
    module:"factory/product_definition",source:file!(),line:line!(),
}, &bilibi_module_registry::contributions::PageWrites {
    service:crate::PAGES,contracts:&[],
    pending:bilibi_module_registry::contributions::Pending::Every {reason:"Resource types, resources, products and formula and routing publication write through the Product Definition facade; no contract yet."},
}) }
