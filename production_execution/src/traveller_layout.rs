use bilibi_module_registry::{Contribution, Provenance};
use bilibi_templates::{
    PrintContract, PrintDefault, PrintFacts, PrintField, PrintKind, PrintValue, PRINT_CATALOGUE,
};

pub(crate) const KEY: &str = "factory.production_execution.traveller";

macro_rules! field {
    ($id:literal, $en:literal, $ms:literal, $kind:ident, $sample:expr) => {
        PrintField {
            id: $id,
            labels: &[("en", $en), ("ms", $ms)],
            kind: PrintKind::$kind,
            optional: false,
            required_output: true,
            sample: $sample,
        }
    };
}
const fn optional(mut field: PrintField) -> PrintField {
    field.optional = true;
    field
}
const COLUMNS: &[PrintField] = &[
    field!(
        "sequence",
        "Sequence",
        "Turutan",
        Integer,
        PrintValue::Integer(10)
    ),
    field!(
        "operation",
        "Operation",
        "Operasi",
        Text,
        PrintValue::Text("FORM-10")
    ),
    optional(field!(
        "instructions",
        "Instructions",
        "Arahan",
        Text,
        PrintValue::Text("Confirm the item dimensions.\nRecord the next inspection.")
    )),
];
const ROW: PrintFacts<'static> = &[
    ("sequence", COLUMNS[0].sample),
    ("operation", COLUMNS[1].sample),
    ("instructions", COLUMNS[2].sample),
];
pub(crate) static CONTRACT: PrintContract = PrintContract {
    key: KEY,
    label: "Work order traveller",
    version: "1",
    base_language: "en",
    states: &["order", "batch"],
    fields: &[
        field!(
            "number",
            "Order reference",
            "Rujukan pesanan",
            Text,
            PrintValue::Text("WO-0042")
        ),
        field!(
            "kind",
            "Work kind",
            "Jenis kerja",
            Text,
            PrintValue::Text("Production order")
        ),
        field!(
            "issuer",
            "Company",
            "Syarikat",
            Text,
            PrintValue::Text("Example Works")
        ),
        field!(
            "product",
            "Product reference",
            "Rujukan produk",
            Integer,
            PrintValue::Integer(21)
        ),
        field!(
            "formula",
            "Formula version",
            "Versi formula",
            Integer,
            PrintValue::Integer(2)
        ),
        field!(
            "routing",
            "Routing version",
            "Versi laluan",
            Integer,
            PrintValue::Integer(3)
        ),
        optional(field!(
            "demand",
            "Demand reference",
            "Rujukan permintaan",
            Text,
            PrintValue::Text("SO-0142")
        )),
        optional(field!(
            "quantity",
            "Target quantity",
            "Kuantiti sasaran",
            Text,
            PrintValue::Text("260 EA")
        )),
        optional(field!(
            "due_on",
            "Due date",
            "Tarikh akhir",
            Date,
            PrintValue::Date(jiff::civil::date(2026, 10, 15))
        )),
        PrintField {
            id: "operations",
            labels: &[],
            kind: PrintKind::Rows(COLUMNS),
            optional: false,
            required_output: true,
            sample: PrintValue::Rows(&[ROW]),
        },
    ],
    defaults: &[
        PrintDefault {
            language: "en",
            version: "1",
            layout: include_str!("../prints/traveller.en.json"),
        },
        PrintDefault {
            language: "ms",
            version: "1",
            layout: include_str!("../prints/traveller.ms.json"),
        },
    ],
};
inventory::submit! { Contribution::new(PRINT_CATALOGUE, KEY, Provenance {
    module:"factory/production_execution",source:file!(),line:line!(),
}, &CONTRACT) }
