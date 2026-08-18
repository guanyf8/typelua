use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use super::{Action, ParseTable};

pub(super) const CELL_ERROR: i32 = 0;
pub(super) const CELL_ACCEPT: i32 = i32::MAX;

impl ParseTable {
    // action + goto 合为一张表
    pub(super) fn dense_cells(&self) -> Vec<i32> {
        let symbol_count = self.symbol_names.len();
        let mut cells = vec![CELL_ERROR; self.state_count * symbol_count];
        let at = |state: u32, symbol: u32| state as usize * symbol_count + symbol as usize;

        for (&(state, symbol), &action) in &self.action {
            cells[at(state, symbol)] = match action {
                // +1 让 0 能专门表示「无动作」
                Action::Shift(target) => target as i32 + 1,
                Action::Reduce(rule) => -(rule as i32) - 1,
                Action::Accept => CELL_ACCEPT,
            };
        }
        for (&(state, symbol), &target) in &self.goto {
            cells[at(state, symbol)] = target as i32 + 1;
        }
        cells
    }

    pub(crate) fn export(&self) -> TokenStream {
        let state_count = self.state_count;
        let symbol_count = self.symbol_names.len();
        let rule_count = self.rule_lhs.len();
        let end_symbol = self.end_symbol;

        let cells = self.dense_cells();
        let symbol_names = self.symbol_names.iter().map(|name| name.as_str());
        let is_terminal = self.is_terminal.iter().copied();
        let rule_lhs = self.rule_lhs.iter().copied();
        let rule_rhs_len = self.rule_rhs_len.iter().copied();
        // 标签名直接当枚举变体名
        let prod_variants: Vec<Ident> = self.label_names.iter()
            .map(|name| Ident::new(name, Span::call_site())).collect();
        let rule_prod: Vec<TokenStream> = self.rule_label.iter()
            .map(|slot| match slot {
                Some(index) => {
                    let variant = &prod_variants[*index as usize];
                    quote! { Some(Prod::#variant) }
                }
                None => quote! { None },
            }).collect();

        quote! {
            pub const NUM_STATES:  usize = #state_count;
            pub const NUM_SYMBOLS: usize = #symbol_count;
            pub const NUM_RULES:   usize = #rule_count;
            // index of $end
            pub const END_SYMBOL:  u32   = #end_symbol;

            pub static SYMBOL_NAMES: [&str; NUM_SYMBOLS] = [#(#symbol_names),*];
            pub static IS_TERMINAL: [bool; NUM_SYMBOLS] = [#(#is_terminal),*];
            // rule_index -> left_symbol_index -> SYMBOL_NAMES[left_index]
            pub static RULE_LHS: [u32; NUM_RULES] = [#(#rule_lhs),*];
            /// rule_index -> right_symbol_count
            pub static RULE_RHS_LEN: [u32; NUM_RULES] = [#(#rule_rhs_len),*];

            // 用 static ，保证 rodata 里只有一份
            static TABLE: [i32; NUM_STATES * NUM_SYMBOLS] = [#(#cells),*];
            const CELL_ERROR: i32 = 0;
            const CELL_ACCEPT: i32 = i32::MAX;

            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            // 标签名由文法作者定，`@local_decl` 这种小写不该在消费方刷警告
            #[allow(non_camel_case_types)]
            pub enum Prod { #(#prod_variants),* }

            /// 没贴标签的产生式是 None
            pub static RULE_PROD: [Option<Prod>; NUM_RULES] = [#(#rule_prod),*];

            #[inline]
            pub fn prod(rule: u32) -> Option<Prod> { RULE_PROD[rule as usize] }
            

            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub enum Action {
                Error,
                Shift(u32 /* target_state */),
                Reduce(u32 /* rule_index */),
                Accept,
            }

            #[inline]
            fn cell(state: u32, symbol: u32) -> i32 {
                TABLE[state as usize * NUM_SYMBOLS + symbol as usize]
            }

            /// 终结符列：查该状态遇到这个终结符时做什么
            pub fn action(state: u32, terminal: u32) -> Action {
                match cell(state, terminal) {
                    CELL_ERROR => Action::Error,
                    // 必须排在 v > 0 之前，否则 i32::MAX 会被当成移进
                    CELL_ACCEPT => Action::Accept,
                    v if v > 0 => Action::Shift((v - 1) as u32),
                    v => Action::Reduce((-v - 1) as u32),
                }
            }

            /// 非终结符列：归约完把左部压回栈时转向哪个状态
            pub fn goto(state: u32, nonterminal: u32) -> Option<u32> {
                match cell(state, nonterminal) {
                    CELL_ERROR => None,
                    v => Some((v - 1) as u32),
                }
            }

            /// 按名字找符号下标。用来把词法器的 token 种类桥接到列下标，只在初始化时调用
            pub fn symbol_index(name: &str) -> Option<u32> {
                SYMBOL_NAMES.iter().position(|&n| n == name).map(|i| i as u32)
            }

            /// 该状态下所有合法的输入终结符，用于「期望以下之一」这类报错
            pub fn expected_terminals(state: u32) -> Vec<&'static str> {
                (0..NUM_SYMBOLS as u32)
                    .filter(|&s| IS_TERMINAL[s as usize] && cell(state, s) != CELL_ERROR)
                    .map(|s| SYMBOL_NAMES[s as usize])
                    .collect()
            }
        }
    }
}
