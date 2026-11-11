use hamondb_macro::Table;

#[derive(Clone, Table)]
struct SimpleTable{
    #[index]
    #[skip]
    id: String,
    var: String,
}

fn main() {}