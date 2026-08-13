use proc_macro2::{TokenStream, TokenTree};
use std::fmt;

#[derive(Eq, PartialEq, Ord, PartialOrd)]
pub struct Rule {
    pub left: u32,
    pub right: Vec<u32>,
}


pub struct Grammar {
    pub rules: Vec<Rule>,
    pub names: Vec<String>,
    pub firsts: Vec<HashSet<i32>>,    //-1意味着为空 //todo 这个数据表示有点丑陋，后面再想想怎么优化
    pub terminals: Vec<bool>,
}


impl Grammar {
    pub fn new() -> Grammar {
        Grammar {
            rules: vec![],
            names: vec![],
            firsts: vec![],      
            terminals: vec![],     
        }
    }

    pub fn build_grammar(input: TokenStream) -> Grammar {
        let tokens: Vec<TokenTree> = input.into_iter().collect();
        let mut grammar = Grammar::new();

        let mut left_buffer: String = String::new();
        let mut buffer: Vec<String> = vec![];

        let mut i = 0;
        while i < tokens.len() {
            let token = &tokens[i];
            match token {
                TokenTree::Punct(punct) => {
                    match punct.as_char() {
                        ':' => {
                            if buffer.len() != 1 {
                                panic!(
                                    "ambiguous ':' : expected exactly one nonterminal on the left-hand side, got {:?}",
                                    buffer
                                );
                            }
                            left_buffer = buffer.pop().unwrap();
                        }
                        '|' => {
                            if left_buffer.is_empty() {
                                panic!("ambiguous '|' : no left-hand side defined yet");
                            }
                            grammar.add_rule(&left_buffer, &buffer);
                            buffer.clear();
                        }
                        ';' => {
                            let next_is_alternative = matches!(
                                tokens.get(i + 1),
                                Some(TokenTree::Punct(p)) if p.as_char() == '|'
                            );
                            let next_is_semicolon = matches!(
                                tokens.get(i + 1),
                                Some(TokenTree::Punct(p)) if p.as_char() == ';'
                            );

                            if next_is_alternative || next_is_semicolon {
                                // Still inside an alternative: ';' is a terminal token.
                                buffer.push(";".to_string());
                            } else {
                                // Rule terminator.
                                if left_buffer.is_empty() {
                                    panic!("ambiguous ';' : no left-hand side defined yet");
                                }
                                grammar.add_rule(&left_buffer, &buffer);
                                buffer.clear();
                                left_buffer.clear();
                            }
                        }
                        _ => {
                            buffer.push(punct.as_char().to_string());
                        }
                    }
                }
                TokenTree::Group(group) => {
                    panic!(
                        "Group {:?} not allowed here, please use plain terminals/nonterminals",
                        group
                    );
                }
                TokenTree::Ident(ident) => {
                    buffer.push(ident.to_string());
                }
                TokenTree::Literal(literal) => {
                    buffer.push(literal.to_string());
                }
            }
            i += 1;
        }

        // Flush the last alternative if the input did not end with a rule terminator.
        if !left_buffer.is_empty() && !buffer.is_empty() {
            grammar.add_rule(&left_buffer, &buffer);
        }

        grammar.terminals = vec![true; grammar.names.len()];
        
        // 标记终止符号
        //todo 这里其实不完全对，需要识别只存在左边却从来没出现过在右边的死规则（有可能是typo）
        grammar.rules.iter().for_each(|rule| {
            grammar.terminals[rule.left as usize] = false;
        });

        grammar.build_first_set();
        

        grammar
    }

    fn build_first_set(&mut self){
        // 协议first中含有-1意味着可为空
        let len = self.names.len();
        self.firsts = vec![HashSet::new(); len];

        //terminal的first是它本身
        for i in 0..len {
            if self.terminals[i] {
                self.firsts[i].insert(i as i32);
            }
        }

        let mut changed = true;
        while changed {
            changed = false;
            for rule in &self.rules {
                let mut all_nullable = true;
                for symbol in &rule.right {
                    let symbol_first: Vec<i32> = self.firsts[*symbol as usize].iter().copied().collect();
                    for first in &symbol_first {
                        if *first != -1 {     //todo 因为数据表示选型，非常容易写错，后面可以想想怎么优化
                            changed |= self.firsts[rule.left as usize].insert(*first);
                        }
                    }
                    if !symbol_first.contains(&-1) {
                        all_nullable = false;
                        break;
                    }
                }
                if all_nullable {
                    changed |= self.firsts[rule.left as usize].insert(-1);
                }
            }

        }
    }

    pub fn get_seq_first(&self, seq: &[u32], lookahead: u32) -> HashSet<u32> {
        let mut seq_first: HashSet<u32> = HashSet::new();
        let mut nullable = true;
        for symbol in seq {
            let symbol_first = &self.firsts[*symbol as usize];
            seq_first.extend(symbol_first.iter().filter_map(|&f| if f != -1 { Some(f as u32) } else { None }));
            if !symbol_first.contains(&-1) {
                nullable = false;
                break;
            }
        }
        if nullable {
            seq_first.insert(lookahead);
        }
        seq_first
    }

    pub fn get_name(&self, index:u32) -> Option<&String> {
        self.names.get(index as usize)
    }

    fn get_index(&self, name:&str) -> Option<u32> {
        self.names.iter().position(|x| x == name).map(|x| x as u32)
    }


    pub fn add_rule(&mut self, left: &String, right: &Vec<String>) -> &Rule {
        let left_index = self.get_index(left).unwrap_or_else(|| {
            self.names.push(left.clone());
            (self.names.len() as u32) - 1
        });
        let right_index= right
            .iter()
            .map(|x| {
                self.get_index(x).unwrap_or_else(|| {
                    self.names.push(x.clone());
                    (self.names.len() as u32) - 1
                })
            })
            .collect();
        self.rules.push(Rule {
            left: left_index,
            right: right_index,
        });
        self.rules.last().unwrap()
    }
}

impl fmt::Display for Grammar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for rule in &self.rules {
            let lhs = self.get_name(rule.left).map(|s| s.as_str()).unwrap_or("???");
            write!(f, "{} ->", lhs)?;
            for &sym in &rule.right {
                let name = self.get_name(sym).map(|s| s.as_str()).unwrap_or("???");
                write!(f, " {}", name)?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone,Copy, PartialEq, PartialOrd,Eq, Ord, Hash)]
struct Item {
    rule_index: usize,
    position: usize,
}

use std::collections::{BTreeSet, HashSet};
type Items = BTreeSet<Item>;

trait WrapItem {
    fn wrap_items(self)->Items;
}

impl WrapItem for Items {
    fn wrap_items(self)->Items{
        self
    }
}

impl WrapItem for Item {
    fn wrap_items(self)->Items{
        let mut items = BTreeSet::new();
        items.insert(self);
        items
    }
}

fn closure<T:WrapItem>(item:T,grammar:&Grammar)->Items{
    let mut items = item.wrap_items();
    let mut todo_list: Vec<Item> = items.iter().copied().collect();
    while let Some(i) = todo_list.pop() {
        let rule=&grammar.rules[i.rule_index];
        let Some(&symbol) = rule.right.get(i.position) else { continue };
        for (rule_index,rule_def) in grammar.rules.iter().enumerate() {
            if rule_def.left == symbol {
                let new_item = Item {
                    rule_index,
                    position: 0,
                };
                // true说明是新项，没看过follow符号
                if items.insert(new_item) {
                    todo_list.push(new_item);
                }
            }
        }
    }
     items
}

fn goto(items: &Items, symbol: u32, grammar: &Grammar) -> Items {
    let mut goto_items = BTreeSet::new();
    for item in items {
        let rule = &grammar.rules[item.rule_index];
        if let Some(&follow) = rule.right.get(item.position) && follow == symbol{
            goto_items.insert(Item {      
                rule_index: item.rule_index,
                position: item.position + 1,
            });
        }
    }
    goto_items       //只输出内核项，不需要闭包
    // closure(goto_items, grammar)
}

type ItemsCollection = Vec<Items>;

fn kernel(grammar:&Grammar)->(ItemsCollection, Vec<(u32/*state */,u32 /* trans_state */, u32 /* symbol */)>){
    let mut kernel_collection: ItemsCollection = Vec::new();
    let mut transitions: Vec<(u32,u32,u32)> = Vec::new();
    //第一条文法 chunk -> block 就是增广文法的开端，以此为接收状态
    let start_item = Item {
        rule_index: 0,
        position: 0,
    };

    let mut pos = 0;
    kernel_collection.push(BTreeSet::from([start_item]));

    while pos < kernel_collection.len() {
        let items = &kernel_collection[pos];
        let items_closure = closure(items.clone(), grammar);
        let concern_symbols = items_closure.iter().filter_map(|item| {
            let rule = &grammar.rules[item.rule_index];
            rule.right.get(item.position)
        });
        for symbol in concern_symbols {
            let goto_items = goto(&items_closure, *symbol, grammar);
            if goto_items.is_empty() { continue; }
            let index = match kernel_collection.iter().position(|s| s == &goto_items) {
                Some(index) => index,
                None => {
                    let index = kernel_collection.len();
                    kernel_collection.push(goto_items.clone());
                    index
                }
            };
            transitions.push((pos as u32, index as u32, *symbol));
        }
        pos +=1;
    }

    (kernel_collection, transitions)
}


// pub struct ParseTable {
//     action:,
//     goto:,
// }

// pub fn generate_parse_table(grammar: &Grammar) -> ParseTable {
// }
