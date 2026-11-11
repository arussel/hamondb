use hamondb_macro::Table;

#[derive(Clone, Table)]
struct SimpleTable{
    #[primary_key]
    #[skip]
    id: String,
    var: String,
}

fn main() {}