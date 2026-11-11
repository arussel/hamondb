use hamondb_macro::Table;

#[derive(Clone, Table)]
struct SimpleTable{
    #[primary_key]
    id: String,
    #[skip]
    #[computed]
    var: String,
}

fn main() {}