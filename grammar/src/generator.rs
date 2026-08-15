use proc_macro2::{TokenStream, TokenTree};
use std::fmt;

// 测试放在本模块的子模块里，这样它能直接看到私有项，不必为测试放宽可见性
#[cfg(test)]
mod tests;

/// 输入结束符的名字。
const END_SYMBOL: &str = "$end";

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

        //早期append一个 $end，不会出现在所有rule中
        grammar.names.push(END_SYMBOL.to_string());

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

    /// 输入结束符 `$end` 的下标
    fn end_symbol(&self) -> u32 {
        self.get_index(END_SYMBOL).expect("END_SYMBOL 应在 build_grammar 中加入")
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
struct Item<L> {
    rule_index: usize,
    position: usize,
    lookahead: L
}

type Item0 = Item<()>;
type Item1 = Item<u32>;

use std::collections::{BTreeSet, HashSet};
type Items<L> = BTreeSet<Item<L>>;

trait Lookahead: Ord + Clone + Copy {
    fn derive(&self, grammar: &Grammar, seq: &[u32]) -> HashSet<Self>;
}

impl Lookahead for u32 {
    fn derive(&self, grammar: &Grammar, seq: &[u32]) -> HashSet<Self> {
        grammar.get_seq_first(seq, *self)
    }
}

impl Lookahead for () {
    fn derive(&self, _grammar: &Grammar, _seq: &[u32]) -> HashSet<Self> {
        let mut s=HashSet::new();
        s.insert(*self);
        s
    }
}

trait WrapItem<T> {
    fn wrap_items(self)->Items<T>;
}

impl<T: Ord + Clone + Copy> WrapItem<T> for Items<T> {
    fn wrap_items(self)->Items<T>{
        self
    }
}

impl<T: Ord + Clone + Copy> WrapItem<T> for Item<T> {
    fn wrap_items(self)->Items<T>{
        let mut items = BTreeSet::new();
        items.insert(self);
        items
    }
}

fn build_closure<'a, T,P>(item: P,grammar:&Grammar)->Items<T>
where T:Lookahead , P : WrapItem<T> {
    let mut items = item.wrap_items();
    let mut todo_list: Vec<Item<T>> = items.iter().copied().collect();
    while let Some(i) = todo_list.pop() {
        let rule=&grammar.rules[i.rule_index];
        let Some(&symbol) = rule.right.get(i.position) else { continue };

        let suffix = &rule.right[i.position+1..];
        let lookahead = i.lookahead.derive(grammar, suffix);

        for (rule_index,rule_def) in grammar.rules.iter().enumerate() {
            if rule_def.left == symbol {
                for &l in &lookahead {
                    let new_item = Item {
                        rule_index,
                        position: 0,
                        lookahead: l,
                    };
                    // true说明是新项，没看过follow符号
                    if items.insert(new_item) {
                        todo_list.push(new_item);
                    }
                }
                
            }
        }
    }
     items
}

fn goto<T:Lookahead>(items: &Items<T>, symbol: u32, grammar: &Grammar) -> Items<T> {
    let mut goto_items = BTreeSet::new();
    for item in items {
        let rule = &grammar.rules[item.rule_index];
        if let Some(&follow) = rule.right.get(item.position) && follow == symbol{
            goto_items.insert(Item {      
                rule_index: item.rule_index,
                position: item.position + 1,
                ..*item
            });
        }
    }
    goto_items       //只输出内核项，不需要闭包
    // build_closure(goto_items, grammar)
}

type ItemsCollection<T> = Vec<Items<T>>;

fn build_kernels(grammar:&Grammar)->(ItemsCollection<()>, HashMap<(u32/*state */,u32 /* symbol */), u32 /* trans_state */>){
    let mut kernel_collection: ItemsCollection<()> = Vec::new();
    let mut transitions: HashMap<(u32,u32), u32> = HashMap::new();
    //第一条文法 chunk -> block 就是增广文法的开端，以此为接收状态
    let start_item = Item {
        rule_index: 0,
        position: 0,
        lookahead: (),
    };

    let mut pos = 0;
    kernel_collection.push(BTreeSet::from([start_item]));

    while pos < kernel_collection.len() {
        let items = &kernel_collection[pos];
        let items_closure = build_closure(items.clone(), grammar);
        let concern_symbols: BTreeSet<&u32> = items_closure.iter().filter_map(|item| {
            let rule = &grammar.rules[item.rule_index];
            rule.right.get(item.position)
        }).collect();
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
            transitions.insert((pos as u32, *symbol), index as u32);
        }
        pos +=1;
    }

    (kernel_collection, transitions)
}

type Lookaheads = Vec<HashMap<Item0,HashSet<u32>>>;
type Propagates = Vec<HashMap<Item0,HashSet<(u32/* kernel_index */, Item0)>>>;
const PROPAGATE_MARK: u32 = u32::MAX;

fn build_lookaheads(kernels: &ItemsCollection<()>, grammar: &Grammar, transitions: &HashMap<(u32,u32),u32>)
                -> (Lookaheads, Propagates) {
    
    // 充分利用LALR和LR(0)的项集内核数量一致、内容一致
    // (kernel_bucket-> Item0) -> lookaheads
    let mut spontaneous:Lookaheads = kernels.iter()
        .map(|kernel|{
            kernel.iter().map(|&item|(item,HashSet::new())).collect()
        }).collect();
    // (kernel_bucket,item0)-> (kernel_bucket,item0)
    let mut propagate:Propagates = vec![HashMap::new(); kernels.len()];
    
    //初始化起始符号
    let start_item = Item0 { rule_index: 0, position: 0, lookahead: () };
    spontaneous[0].get_mut(&start_item)
        .expect("kernel 0 should contains start_item rule[0]")
        .insert(grammar.end_symbol());

    for (i, items) in kernels.iter().enumerate() {
        for item in items {
            let lalr_closure = build_closure(Item {
                rule_index: item.rule_index,
                position: item.position,
                lookahead: PROPAGATE_MARK,
            }, grammar);
            for closure_item in lalr_closure {
                let Some(&concern_symbol) = grammar.rules[closure_item.rule_index].right.get(closure_item.position) else {continue;};
                let goto_index = transitions[&(i as u32, concern_symbol)];
                let target_item:Item0 = Item0 {
                    rule_index: closure_item.rule_index,
                    position: closure_item.position + 1,
                    lookahead: (),
                };
                if closure_item.lookahead == PROPAGATE_MARK {
                    propagate[i].entry(*item).or_default()
                        .insert((goto_index, target_item));
                }else{
                    spontaneous[goto_index as usize].get_mut(&target_item).unwrap()
                        .insert(closure_item.lookahead);
                }
            }
        }
    }

    // # 只是求前看集时的占位符，不能作为真实前看符号外泄 —— 它会变成 ACTION 表里
    // 一个越界的列下标。检查输出而不是分支条件，这样改动分支结构也守得住。
    debug_assert!(
        spontaneous.iter().all(|m| m.values().all(|s| !s.contains(&PROPAGATE_MARK))),
        "# 外泄到前看集里了"
    );

    (spontaneous, propagate)
}
fn resolve_lookaheads(mut spon: Lookaheads, prop: &Propagates) -> Lookaheads {
    let mut todo_list: Vec<(usize, Item0, u32)> = spon
        .iter().enumerate().flat_map(|(i, from_kernel)| {
            from_kernel.iter().flat_map(move |(&from_item, lookaheads)| {
                lookaheads.iter().map(move |&lookahead| (i, from_item, lookahead))
            })
        }).collect();

    while let Some((kernel_index, from_item, lookahead)) = todo_list.pop() {
        let Some(targets) = prop[kernel_index].get(&from_item) else { continue };
        for &(to_kernel, to_item) in targets {
            //不要担心这里的unwrap，前面已经稠密化了，肯定会有的
            let to_lookaheads = spon[to_kernel as usize].get_mut(&to_item).unwrap();
            if to_lookaheads.insert(lookahead) {
                todo_list.push((to_kernel as usize, to_item, lookahead));
            }
        }
    }
    spon
}

fn build_lalr_kernels(kernels: &ItemsCollection<()>, lookaheads: &Lookaheads) -> ItemsCollection<u32> {
    kernels.iter().enumerate().map(move |(i, kernel)| {
        kernel.iter().flat_map(move |item| {
            //不要担心这里的unwrap，前面已经稠密化了，肯定会有的
            lookaheads[i].get(item).unwrap().iter().map(move |&lookahead| {
                Item1 {
                    rule_index: item.rule_index,
                    position: item.position,
                    lookahead,
                }
            })
        }).collect()
    }).collect()
}


enum Action {
    Shift(u32),
    Reduce(u32),
    Accept
}

use std::collections::HashMap;
pub struct ParseTable {
    action:HashMap<(u32/* state */,u32 /* symbol */),Action>,
    goto:HashMap<(u32/* state */,u32 /* symbol */),u32/*state */>,
}

pub fn generate_parse_table(grammar: &Grammar) -> ParseTable {
    let (kernels_lr0, transitions) = build_kernels(grammar);
    let (spontaneous, propagate) = build_lookaheads(&kernels_lr0, grammar, &transitions);
    let lookaheads = resolve_lookaheads(spontaneous, &propagate);
    let kernels_lalr = build_lalr_kernels(&kernels_lr0, &lookaheads);
    let lalr_items = kernels_lalr.iter().map(|kernel|{
        build_closure(kernel.clone(), grammar)
    }).collect::<ItemsCollection<u32>>();
    let mut parse_table = ParseTable {
        action: HashMap::new(),
        goto: HashMap::new(),
    };
    let set_action = |table: &mut HashMap<(u32, u32), Action>,
                                                                state: u32, symbol: u32, action: Action| {
        fn label(a: &Action) -> &'static str {
            match a {
                Action::Shift(_) => "shift",
                Action::Reduce(_) => "reduce",
                Action::Accept => "accept",
            }
        }
        if let Some(a) = table.get(&(state, symbol)){
            match (a, &action) {
                //扔掉reduce就好了
                (Action::Shift(_), Action::Reduce(_)) => {}
                //reduce -reduce冲突不允许
                (Action::Reduce(kept), Action::Reduce(dropped)) => panic!(
                    "reduce-reduce conflicts：state {state} with lookahead `{}` 时，rule {kept} - rule {dropped} ",
                    grammar.names[symbol as usize],
                ),
                (old, new) => unreachable!(
                    "state {state} : `{}` illegal， given {} be covered by {}",
                    grammar.names[symbol as usize], label(old), label(new),
                ),
            }
        }else {
            table.insert((state, symbol), action);
        }
    };

    //贪婪，shift-reduce冲突时优先shift
    for ((state,symbol),target) in transitions {
        if grammar.terminals[symbol as usize] {
            set_action(&mut parse_table.action, 
                state, symbol, Action::Shift(target));
        } else {
            parse_table.goto.insert((state, symbol), target);
        }
    }

    for (i,items) in lalr_items.iter().enumerate() {
        for item in items {
            //有后继
            if item.position< grammar.rules[item.rule_index].right.len(){
                continue;
            }

            if item.rule_index == 0 && item.lookahead == grammar.end_symbol(){
                set_action(&mut parse_table.action, 
                    i as u32, item.lookahead, Action::Accept);
            }else{
                set_action(&mut parse_table.action, 
                    i as u32, item.lookahead, Action::Reduce(item.rule_index as u32));
            }
        }
    }
    parse_table

}
