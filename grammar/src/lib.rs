use proc_macro::TokenStream;
mod generator;
use generator::{Grammar, ParseTable};

#[proc_macro]
pub fn grammar(input: TokenStream) -> TokenStream {
    let grammar = Grammar::build_grammar(input.into());

    let parser_table = ParseTable::generate_parse_table(&grammar);

    TokenStream::from(parser_table)
}

// proc_macro2 -> proc_macro::TokenStream
impl From<ParseTable> for TokenStream {
    fn from(table: ParseTable) -> Self {
        table.export().into()
    }
}
