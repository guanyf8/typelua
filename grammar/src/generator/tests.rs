use super::*;
// 格子编码的哨兵住在兄弟模块 export 里，测试要用同一份定义而不是硬编码 0 / i32::MAX
use super::export::{CELL_ACCEPT, CELL_ERROR};
use quote::quote;
use std::collections::HashSet;

// ============ 测试文法目录 ============
//
// 这个 crate 是通用的 LALR(1) 生成器，不绑定任何具体语言，所以测试用的都是小到能
// 手算的文法，每条针对一个机制。形式不变量在整个目录上循环验证 —— 这比在一条大文法上
// 跑一遍更强：空产生式、纯左递归、单规则这些边界，大文法碰巧覆盖不到。
//
// 增广产生式必须由文法自己提供且唯一（build_grammar 会检查），所以每条文法的第一条
// 规则都是 `start : <真正的起点> ;` 这个形状。

/// 最小可能的文法：一条规则、一个终结符
fn minimal() -> Grammar {
    Grammar::build_grammar(quote! { start : A ; })
}

/// 传递可空性：a 自己没写 ε 候选，但 b 和 c 都可空，所以 a 也可空
fn nullable_chain() -> Grammar {
    Grammar::build_grammar(quote! {
        start : a ;
        a : b c ;
        b : B | ;
        c : C | ;
    })
}

/// 左递归 + ε。FIRST(list) 必须含 X —— 只看右部首符号的实现会漏掉它，
/// 因为首符号 list 自己可空，X 藏在第二个位置
fn left_recursive() -> Grammar {
    Grammar::build_grammar(quote! {
        start : list ;
        list : | list X ;
    })
}

/// ε 产生式的完成项 `[opt -> ·]` position 是 0，**不在任何内核里**，只在闭包里出现。
/// 填表时必须对每个状态求一次 LALR 闭包才能拿到它的归约动作
fn epsilon_reduce() -> Grammar {
    Grammar::build_grammar(quote! {
        start : A opt B ;
        opt : | C ;
    })
}

/// LALR 状态合并：`[n -> N ·]` 从 X 和 Z 两条路都能到达，两条路的后继不同（Y / W）。
/// LR(0) 把它们合成同一个状态，所以前看集必须是并集 `{W, Y}` —— 正统 LR(1) 里
/// 这会是两个独立状态，各带 {Y} / {W}
fn merge() -> Grammar {
    Grammar::build_grammar(quote! {
        start : a_b ;
        a_b : X n Y | Z n W ;
        n : N ;
    })
}

/// FIRST 的不动点必须真的迭代到收敛。规则按 `start -> a -> b -> c -> d -> D` 的顺序写，
/// 而单趟扫描里信息只能沿规则顺序**逆向**流动一格（rule 2 在 rule 3 之前跑，读到的还是
/// 上一轮的 FIRST），所以这条链要 5 轮才能把 D 传到 start。
///
/// 目录里别的文法链都短，3 轮就收敛了 —— 少了这条，「不动点提前停」这类改动测不出来
fn deep_first_chain() -> Grammar {
    Grammar::build_grammar(quote! {
        start : a ;
        a : b ;
        b : c ;
        c : d ;
        d : D ;
    })
}

/// 可空性不能误传：opt 可空但 R 必需，所以 s 和 start 都**不**可空。
/// `build_first_set` 里那道 `if *first != -1` 的闸就是干这个的 —— 少了它，
/// 右部可空符号的 ε 标记会被原样拷进左部，s 被误判成可空，
/// 进而让 LALR 前看集多算出一堆本不该继承的符号。
///
/// 这个形状是必需的：`nullable_chain` 里左部本来就可空，`epsilon_reduce` 的首符号
/// 是终结符会立刻截断，两者都盖不住这条闸
fn required_after_nullable() -> Grammar {
    Grammar::build_grammar(quote! {
        start : s ;
        s : opt R ;
        opt : | O ;
    })
}

/// 前看集传播：n 的后缀是 `b`，而 b 可空，所以 n 的前看符号要从上层继承 ——
/// 走的是 propagate 边而不是自发生成，必须跑完不动点才拿得到
fn propagate() -> Grammar {
    Grammar::build_grammar(quote! {
        start : a ;
        a : n b ;
        b : | B ;
        n : N ;
    })
}

/// 悬空 else：在 `IF stmt · ELSE` 处既能归约 `stmt -> IF stmt`，又能移进 ELSE。
/// 贪婪策略必须保留移进（else 绑到最近的 if），这是全表唯一一处冲突
fn dangling_else() -> Grammar {
    Grammar::build_grammar(quote! {
        start : stmt ;
        stmt : IF stmt | IF stmt ELSE stmt | S ;
    })
}

/// 归约-归约冲突：goto(I0, C) 同时含 `[a -> C ·]` 和 `[b -> C ·]`，两者前看都是 {X}。
/// 这种冲突没有合理的默认解，建表必须 panic。**不进目录**，因为它按设计跑不通
fn reduce_reduce() -> Grammar {
    Grammar::build_grammar(quote! {
        start : s ;
        s : a X | b X ;
        a : C ;
        b : C ;
    })
}

/// 形式不变量逐条跑一遍的文法集合
fn catalogue() -> Vec<(&'static str, Grammar)> {
    vec![
        ("minimal", minimal()),
        ("nullable_chain", nullable_chain()),
        ("left_recursive", left_recursive()),
        ("epsilon_reduce", epsilon_reduce()),
        ("required_after_nullable", required_after_nullable()),
        ("deep_first_chain", deep_first_chain()),
        ("merge", merge()),
        ("propagate", propagate()),
        ("dangling_else", dangling_else()),
    ] as Vec<(&'static str, Grammar)>
}

// ============ 辅助函数 ============

fn sym(grammar: &Grammar, name: &str) -> u32 {
    grammar
        .names
        .iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("symbol {name} not in grammar")) as u32
}

/// FIRST 集里 -1 表示可空，渲染成 ε
fn render(grammar: &Grammar, f: i32) -> String {
    if f == -1 {
        "ε".to_string()
    } else {
        grammar.names[f as usize].clone()
    }
}

/// FIRST 集翻成名字并排序，便于断言
fn first_of(grammar: &Grammar, name: &str) -> Vec<String> {
    let mut names: Vec<String> = grammar.firsts[sym(grammar, name) as usize]
        .iter()
        .map(|&f| render(grammar, f))
        .collect();
    names.sort();
    names
}

/// 算 FIRST(seq · lookahead)，结果翻成名字并排序
fn seq_first_of(grammar: &Grammar, seq: &[&str], lookahead: &str) -> Vec<String> {
    let indices: Vec<u32> = seq.iter().map(|n| sym(grammar, n)).collect();
    let mut names: Vec<String> = grammar
        .get_seq_first(&indices, sym(grammar, lookahead))
        .iter()
        .map(|&f| render(grammar, f as i32))
        .collect();
    names.sort();
    names
}

/// 所有可空符号的名字，排序
fn nullable_symbols(grammar: &Grammar) -> Vec<&str> {
    let mut names: Vec<&str> = grammar
        .firsts
        .iter()
        .enumerate()
        .filter(|(_, first)| first.contains(&-1))
        .map(|(i, _)| grammar.names[i].as_str())
        .collect();
    names.sort();
    names
}

/// 渲染成 `左部 -> 右部`，ε 产生式渲染成 `左部 -> ε`
fn render_rule(grammar: &Grammar, rule_index: usize) -> String {
    let rule = &grammar.rules[rule_index];
    let lhs = &grammar.names[rule.left as usize];
    if rule.right.is_empty() {
        return format!("{lhs} -> ε");
    }
    let rhs: Vec<&str> = rule.right.iter().map(|&s| grammar.names[s as usize].as_str()).collect();
    format!("{lhs} -> {}", rhs.join(" ")) as String
}

/// 跑完整条前看集链路：LR(0) 内核 -> 自发/传播 -> 不动点
fn lalr_lookaheads(grammar: &Grammar) -> Lookaheads {
    let (kernels, transitions) = build_kernels(grammar);
    let (spontaneous, propagate) = build_lookaheads(&kernels, grammar, &transitions);
    resolve_lookaheads(spontaneous, &propagate)
}

/// 取某个内核项的前看集，翻成名字并排序
fn lookaheads_of(
    grammar: &Grammar,
    table: &Lookaheads,
    state: usize,
    rule_index: usize,
    position: usize,
) -> Vec<String> {
    let item = Item0 { rule_index, position, lookahead: () };
    let mut names: Vec<String> = table[state][&item]
        .iter()
        .map(|&t| grammar.names[t as usize].clone())
        .collect();
    names.sort();
    names
}

/// 找出唯一含指定内核项的状态下标
fn state_with(table: &Lookaheads, rule_index: usize, position: usize) -> usize {
    let item = Item0 { rule_index, position, lookahead: () };
    let hits: Vec<usize> = table
        .iter()
        .enumerate()
        .filter(|(_, m)| m.contains_key(&item))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits.len(), 1, "规则 {rule_index} 位置 {position} 的项应只在一个状态里，实际 {hits:?}");
    hits[0]
}

fn count_actions(table: &ParseTable) -> (usize, usize, usize) {
    let mut shift = 0;
    let mut reduce = 0;
    let mut accept = 0;
    for action in table.action.values() {
        match action {
            Action::Shift(_) => shift += 1,
            Action::Reduce(_) => reduce += 1,
            Action::Accept => accept += 1,
        }
    }
    (shift, reduce, accept)
}

/// 所有能在闭包里出现的规则下标。ε 产生式只有 `[r, 0]` 一个项，永远进不了内核，
/// 所以判可达性必须看闭包而不是内核
fn reachable_rules(grammar: &Grammar) -> HashSet<usize> {
    let (kernels, _) = build_kernels(grammar);
    kernels
        .iter()
        .flat_map(|kernel| build_closure::<(), _>(kernel.clone(), grammar))
        .map(|item| item.rule_index)
        .collect()
}

/// 重跑一遍归约侧：凡是想写进已有 Shift 格子的归约，都是被贪婪策略吞掉的移进-归约冲突。
/// 表本身不记录这些，所以要在这里独立复算一遍
fn shift_reduce_conflicts(grammar: &Grammar, table: &ParseTable) -> Vec<String> {
    let (kernels_lr0, transitions) = build_kernels(grammar);
    let (spontaneous, propagate) = build_lookaheads(&kernels_lr0, grammar, &transitions);
    let lookaheads = resolve_lookaheads(spontaneous, &propagate);

    let mut conflicts = Vec::new();
    for (state, kernel) in build_lalr_kernels(&kernels_lr0, &lookaheads).iter().enumerate() {
        for item in build_closure::<u32, _>(kernel.clone(), grammar) {
            if item.position != grammar.rules[item.rule_index].right.len() {
                continue;
            }
            if matches!(table.action.get(&(state as u32, item.lookahead)), Some(Action::Shift(_))) {
                conflicts.push(format!(
                    "前看 {} 时丢掉归约 [{}]",
                    grammar.names[item.lookahead as usize],
                    render_rule(grammar, item.rule_index)
                ));
            }
        }
    }
    conflicts.sort();
    conflicts
}

// ============ DSL 解析 ============

#[test]
fn symbol_indices_follow_first_appearance() {
    let grammar = minimal();
    // 左部先入表，然后是右部，最后 build_grammar 追加 $end
    assert_eq!(grammar.names, ["start", "A", "$end"]);
}

#[test]
fn char_literal_terminals_keep_their_quotes() {
    // DSL 里 '(' 是字符字面量，走 TokenTree::Literal，Literal::to_string 保留引号。
    // 所以文法里标点终结符的名字是 `'('` 而不是 `(` —— 消费方桥接 token 时要按这个对齐
    let grammar = Grammar::build_grammar(quote! {
        start : s ;
        s : '(' N ')' ;
    });
    assert!(grammar.names.contains(&"'('".to_string()), "names = {:?}", grammar.names);
    assert!(grammar.names.contains(&"')'".to_string()));
    assert!(!grammar.names.contains(&"(".to_string()), "不该有不带引号的版本");
}

#[test]
fn bare_punct_terminals_have_no_quotes() {
    // 裸标点走 TokenTree::Punct，名字是单个字符本身，和字符字面量的形式不同
    let grammar = Grammar::build_grammar(quote! {
        start : s ;
        s : N + N ;
    });
    assert!(grammar.names.contains(&"+".to_string()), "names = {:?}", grammar.names);
    assert!(!grammar.names.contains(&"'+'".to_string()));
}

#[test]
fn bare_semicolon_can_be_a_terminal() {
    // `;` 在 DSL 里既是产生式终止符又可能是终结符，判据是看下一个 token：
    // 后面紧跟 `|` 时，这个 `;` 属于候选式内容而不是终止符
    let grammar = Grammar::build_grammar(quote! {
        start : s ;
        s : ; | X ;
    });
    assert!(grammar.names.contains(&";".to_string()), "names = {:?}", grammar.names);
    let s = sym(&grammar, "s");
    let alternatives: Vec<String> = (0..grammar.rules.len())
        .filter(|&i| grammar.rules[i].left == s)
        .map(|i| render_rule(&grammar, i))
        .collect();
    assert_eq!(alternatives, ["s -> ;", "s -> X"]);
}

#[test]
fn bare_semicolon_before_the_terminator_is_also_a_terminal() {
    // 另一半判据：后面紧跟另一个 `;` 时，前一个是终结符、后一个才是终止符
    let grammar = Grammar::build_grammar(quote! {
        start : s ;
        s : ; ;
    });
    let s = sym(&grammar, "s");
    let alternatives: Vec<String> = (0..grammar.rules.len())
        .filter(|&i| grammar.rules[i].left == s)
        .map(|i| render_rule(&grammar, i))
        .collect();
    assert_eq!(alternatives, ["s -> ;"]);
}

#[test]
fn last_alternative_is_flushed_without_a_trailing_terminator() {
    // 输入没有以 `;` 收尾时，最后一条候选式要靠 build_grammar 末尾那次 flush 补上
    let grammar = Grammar::build_grammar(quote! { start : A });
    assert_eq!(grammar.rules.len(), 1);
    assert_eq!(render_rule(&grammar, 0), "start -> A");
    assert_eq!(grammar.names, ["start", "A", "$end"]);
}

#[test]
#[should_panic(expected = "empty grammar")]
fn a_dangling_colon_does_not_invent_a_rule() {
    // 左部有了但右部一个符号都没读到 —— 收尾的 flush 不该凭空造出一条 ε 产生式。
    // 什么都没造出来，就落到空文法的诊断上
    Grammar::build_grammar(quote! { start : });
}

#[test]
fn end_symbol_is_appended_last_and_marked_terminal() {
    for (label, grammar) in catalogue() {
        let last = grammar.names.len() - 1;
        assert_eq!(grammar.names[last], "$end", "{label}: $end 应当在末尾");
        assert!(grammar.terminals[last], "{label}: $end 应当是终结符");
        assert_eq!(grammar.end_symbol() as usize, last, "{label}: end_symbol() 应当指向它");
        // $end 不出现在任何产生式右部
        for rule in &grammar.rules {
            assert!(
                !rule.right.contains(&(last as u32)),
                "{label}: $end 不该出现在产生式右部"
            );
        }
    }
}

#[test]
fn left_hand_sides_are_nonterminals_and_everything_else_is_terminal() {
    for (label, grammar) in catalogue() {
        let lhs: HashSet<u32> = grammar.rules.iter().map(|rule| rule.left).collect();
        for symbol in 0..grammar.names.len() as u32 {
            let expected_terminal = !lhs.contains(&symbol);
            assert_eq!(
                grammar.terminals[symbol as usize], expected_terminal,
                "{label}: 符号 {} 的终结符标记不对",
                grammar.names[symbol as usize]
            );
        }
    }
}

#[test]
fn empty_alternative_becomes_a_zero_length_rule() {
    let grammar = left_recursive();
    // list : | list X ; —— 第一条候选式是空的
    assert_eq!(render_rule(&grammar, 1), "list -> ε");
    assert_eq!(grammar.rules[1].right.len(), 0);
}

#[test]
fn display_renders_every_rule() {
    assert_eq!(minimal().to_string(), "start -> A\n");
    assert_eq!(
        left_recursive().to_string(),
        "start -> list\nlist ->\nlist -> list X\n"
    );
}

#[test]
#[should_panic(expected = "ambiguous ':'")]
fn rejects_multiple_symbols_on_the_left_of_colon() {
    Grammar::build_grammar(quote! { a b : C ; });
}

#[test]
#[should_panic(expected = "ambiguous '|'")]
fn rejects_alternative_without_left_hand_side() {
    Grammar::build_grammar(quote! { | A ; });
}

#[test]
#[should_panic(expected = "ambiguous ';'")]
fn rejects_rule_terminator_without_left_hand_side() {
    Grammar::build_grammar(quote! { A ; });
}

#[test]
#[should_panic(expected = "Group")]
fn rejects_group_tokens() {
    Grammar::build_grammar(quote! { start : ( A ) ; });
}

#[test]
#[should_panic(expected = "empty grammar")]
fn rejects_empty_grammar() {
    Grammar::build_grammar(quote! {});
}

#[test]
#[should_panic(expected = "has 2 alternatives")]
fn rejects_start_symbol_with_multiple_alternatives() {
    // 起点不出现在任何产生式右部，所以它的第二条候选式永远进不了闭包，会被静默丢掉。
    // 增广产生式必须由文法自己提供且唯一
    Grammar::build_grammar(quote! {
        start : a X | b X ;
        a : C ;
        b : C ;
    });
}

// ============ FIRST 集 ============

#[test]
fn terminal_first_is_itself_and_never_nullable() {
    for (label, grammar) in catalogue() {
        for (i, &is_terminal) in grammar.terminals.iter().enumerate() {
            if is_terminal {
                assert_eq!(
                    grammar.firsts[i],
                    HashSet::from([i as i32]),
                    "{label}: 终结符 {} 的 FIRST 应当只有自己",
                    grammar.names[i]
                );
            }
        }
    }
}

#[test]
fn first_never_contains_a_nonterminal() {
    for (label, grammar) in catalogue() {
        for (i, first) in grammar.firsts.iter().enumerate() {
            for &f in first {
                assert!(
                    f == -1 || grammar.terminals[f as usize],
                    "{label}: FIRST({}) 混入了非终结符 {}",
                    grammar.names[i],
                    grammar.names[f as usize]
                );
            }
        }
    }
}

#[test]
fn nullability_is_transitive() {
    let grammar = nullable_chain();
    // b 和 c 各写了 ε 候选；a -> b c 两个都可空所以 a 可空；start -> a 跟着可空。
    // 终结符永不可空，所以可空集恰好是这四个非终结符
    assert_eq!(nullable_symbols(&grammar), ["a", "b", "c", "start"]);
}

#[test]
fn first_crosses_nullable_symbols() {
    let grammar = nullable_chain();
    assert_eq!(first_of(&grammar, "b"), ["B", "ε"]);
    assert_eq!(first_of(&grammar, "c"), ["C", "ε"]);
    // a -> b c：b 可空，所以要跨过它看到 FIRST(c)
    assert_eq!(first_of(&grammar, "a"), ["B", "C", "ε"]);
    assert_eq!(first_of(&grammar, "start"), ["B", "C", "ε"]);
}

#[test]
fn first_reaches_the_fixed_point_along_a_deep_chain() {
    let grammar = deep_first_chain();
    // 链上每一环的 FIRST 都必须是 {D}。规则顺序和信息流动方向相反，
    // 所以这要求不动点迭代 5 轮 —— 提前停就会留下空的 FIRST
    for name in ["start", "a", "b", "c", "d"] {
        assert_eq!(first_of(&grammar, name), ["D"], "FIRST({name}) 没传到位");
    }
    // 链上没有 ε 产生式，所以谁都不可空
    assert!(nullable_symbols(&grammar).is_empty());
}

#[test]
fn nullability_does_not_leak_through_a_required_symbol() {
    let grammar = required_after_nullable();
    // s : opt R —— opt 可空，但 R 必需，所以 s 不可空，start 跟着不可空。
    // 可空集恰好只有 opt
    assert_eq!(nullable_symbols(&grammar), ["opt"]);
    assert_eq!(first_of(&grammar, "opt"), ["O", "ε"]);
    // FIRST(s) 要跨过可空的 opt 看到 R，但自己**不含** ε
    assert_eq!(first_of(&grammar, "s"), ["O", "R"]);
    assert_eq!(first_of(&grammar, "start"), ["O", "R"]);
}

#[test]
fn left_recursion_does_not_hide_symbols_after_the_recursive_call() {
    let grammar = left_recursive();
    // list : ε | list X —— 第二条候选式的首符号是 list 自己且可空，
    // 只看右部第一个符号的实现会漏掉 X
    assert_eq!(first_of(&grammar, "list"), ["X", "ε"]);
}

// ============ get_seq_first ============

#[test]
fn seq_first_of_empty_suffix_is_just_the_lookahead() {
    let grammar = nullable_chain();
    // 点走到产生式末尾时后缀为空：FIRST(ε · a) = {a}
    assert_eq!(seq_first_of(&grammar, &[], "B"), ["B"]);
}

#[test]
fn seq_first_stops_at_the_first_non_nullable_symbol() {
    let grammar = nullable_chain();
    // b 可空、C 不可空 ⇒ 继承来的前看符号 B 不并入
    assert_eq!(seq_first_of(&grammar, &["b", "C"], "B"), ["B", "C"]);
    // 反过来：C 在前就直接截断，b 的 FIRST 看不到
    assert_eq!(seq_first_of(&grammar, &["C", "b"], "B"), ["C"]);
}

#[test]
fn seq_first_folds_in_lookahead_when_suffix_is_fully_nullable() {
    let grammar = nullable_chain();
    // 后缀 b c 整体可空 ⇒ 必须并入继承来的前看符号。这里故意用一个
    // 不在 FIRST(b c) 里的符号，好区分「并入了」和「本来就有」
    let mut expected = seq_first_of(&grammar, &["b", "c"], "$end");
    expected.sort();
    assert_eq!(expected, ["$end", "B", "C"]);
}

#[test]
fn seq_first_dedups_a_lookahead_already_in_the_suffix() {
    let grammar = nullable_chain();
    // b c 整体可空且 B ∈ FIRST(b c)，前看符号也是 B：
    // 结果里 B 只能有一份，否则前看集无法用 == 比较（LALR 合并状态依赖这一点）
    let seq = [sym(&grammar, "b"), sym(&grammar, "c")];
    let result = grammar.get_seq_first(&seq, sym(&grammar, "B"));
    assert_eq!(result.len(), 2, "应当只有 B 和 C：{result:?}");
    assert!(result.contains(&sym(&grammar, "B")));
    assert!(result.contains(&sym(&grammar, "C")));
}

// ============ LR(0) 自动机 ============

#[test]
fn start_state_kernel_is_exactly_the_augmented_item() {
    for (label, grammar) in catalogue() {
        let (states, _) = build_kernels(&grammar);
        let start: Vec<_> = states[0].iter().collect();
        assert_eq!(start.len(), 1, "{label}: 起始内核只该有一项");
        assert_eq!(start[0].rule_index, 0, "{label}: 起始项必须是规则 0");
        assert_eq!(start[0].position, 0, "{label}: 点在最左");
    }
}

#[test]
fn lr0_automaton_is_well_formed() {
    for (label, grammar) in catalogue() {
        let (states, transitions) = build_kernels(&grammar);
        // (状态, 符号) -> 目标 的唯一性由 HashMap 的类型保证，这里查值域
        for (&(from, symbol), &to) in &transitions {
            assert!(
                (to as usize) < states.len(),
                "{label}: 转移 ({from}, {}) 的目标 {to} 越界",
                grammar.names[symbol as usize]
            );
            assert!((from as usize) < states.len(), "{label}: 转移源 {from} 越界");
        }
        // 除起始态外，每个状态都必须至少被一条转移指向，否则是不可达的死状态
        let reached: HashSet<u32> = transitions.values().copied().collect();
        for i in 1..states.len() as u32 {
            assert!(reached.contains(&i), "{label}: 状态 {i} 不可达");
        }
    }
}

#[test]
fn every_rule_reaches_the_automaton() {
    // 一条规则如果连闭包都进不去，它就是死规则 —— 要么文法写错，要么生成器漏了。
    // 起始符号多候选式那个坑就属于这一类，现在由 build_grammar 直接拒绝
    for (label, grammar) in catalogue() {
        let reachable = reachable_rules(&grammar);
        let dead: Vec<String> = (0..grammar.rules.len())
            .filter(|i| !reachable.contains(i))
            .map(|i| render_rule(&grammar, i))
            .collect();
        assert!(dead.is_empty(), "{label}: 这些规则从未出现在任何闭包里：{dead:?}");
    }
}

#[test]
fn there_is_exactly_one_accepting_state() {
    for (label, grammar) in catalogue() {
        let (states, _) = build_kernels(&grammar);
        let accepting: Vec<usize> = states
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.iter()
                    .any(|i| i.rule_index == 0 && i.position == grammar.rules[0].right.len())
            })
            .map(|(idx, _)| idx)
            .collect();
        assert_eq!(accepting.len(), 1, "{label}: 接受状态应当唯一，实际 {accepting:?}");
    }
}

#[test]
fn minimal_grammar_automaton_is_exactly_two_states() {
    let grammar = minimal();
    let (states, transitions) = build_kernels(&grammar);
    // I0 = {start -> · A}，A 是终结符所以闭包不扩张；goto(I0, A) = {start -> A ·}
    assert_eq!(states.len(), 2);
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[&(0, sym(&grammar, "A"))], 1);
}

// ============ LALR(1) 前看集 ============

#[test]
fn lalr_merges_lookaheads_from_two_contexts() {
    let grammar = merge();
    let table = lalr_lookaheads(&grammar);
    // 规则 3 是 n -> N，位置 1 即完成项 [n -> N ·]
    let state = state_with(&table, 3, 1);
    assert_eq!(
        lookaheads_of(&grammar, &table, state, 3, 1),
        ["W", "Y"],
        "两个上下文的前看符号必须并起来"
    );
}

#[test]
fn lookaheads_propagate_through_a_nullable_suffix() {
    let grammar = propagate();
    let table = lalr_lookaheads(&grammar);
    // 规则 4 是 n -> N。它的后缀是 b，FIRST(b) = {B, ε}：
    //   B    自发生成（来自 FIRST(b)）
    //   $end 必须沿传播边从起始项继承过来，因为 b 可空
    let state = state_with(&table, 4, 1);
    assert_eq!(
        lookaheads_of(&grammar, &table, state, 4, 1),
        ["$end", "B"],
        "$end 只能靠传播拿到 —— 少了它说明不动点没跑"
    );
}

#[test]
fn propagation_is_what_supplies_the_inherited_lookahead() {
    // 上一条的反证：不跑 resolve_lookaheads，$end 就不在里面
    let grammar = propagate();
    let (kernels, transitions) = build_kernels(&grammar);
    let (spontaneous, _) = build_lookaheads(&kernels, &grammar, &transitions);
    let state = state_with(&spontaneous, 4, 1);
    let item = Item0 { rule_index: 4, position: 1, lookahead: () };
    assert!(
        !spontaneous[state][&item].contains(&grammar.end_symbol()),
        "自发前看集里不该有 $end，它是继承来的"
    );
}

#[test]
fn accept_item_sees_only_the_end_symbol() {
    for (label, grammar) in catalogue() {
        let table = lalr_lookaheads(&grammar);
        let position = grammar.rules[0].right.len();
        let state = state_with(&table, 0, position);
        assert_eq!(
            lookaheads_of(&grammar, &table, state, 0, position),
            ["$end"],
            "{label}: 接受项只应在读到输入结束时归约"
        );
    }
}

#[test]
fn every_kernel_item_has_a_nonempty_lookahead_set() {
    // 前看集为空的内核项永远无法参与归约，而 build_lalr_kernels 会把它静默丢掉
    for (label, grammar) in catalogue() {
        let table = lalr_lookaheads(&grammar);
        for (state, items) in table.iter().enumerate() {
            for (item, lookaheads) in items {
                assert!(
                    !lookaheads.is_empty(),
                    "{label}: 状态 {state} 的项 (规则 {}, 位置 {}) 前看集为空",
                    item.rule_index,
                    item.position
                );
            }
        }
    }
}

#[test]
fn lookaheads_are_all_valid_terminals() {
    // 守两件事：传播标记 u32::MAX 没有外泄，且没有非终结符混进前看集
    for (label, grammar) in catalogue() {
        let table = lalr_lookaheads(&grammar);
        for items in &table {
            for (item, lookaheads) in items {
                for &la in lookaheads {
                    assert!(
                        (la as usize) < grammar.names.len(),
                        "{label}: 前看符号下标 {la} 越界（规则 {}, 位置 {}）",
                        item.rule_index,
                        item.position
                    );
                    assert!(
                        grammar.terminals[la as usize],
                        "{label}: 非终结符 {} 混进了前看集",
                        grammar.names[la as usize]
                    );
                }
            }
        }
    }
}

#[test]
fn lalr_kernels_expand_one_item_per_lookahead() {
    let grammar = merge();
    let (kernels, transitions) = build_kernels(&grammar);
    let (spontaneous, propagate) = build_lookaheads(&kernels, &grammar, &transitions);
    let lookaheads = resolve_lookaheads(spontaneous, &propagate);
    let lalr = build_lalr_kernels(&kernels, &lookaheads);

    // 每个内核项摊成 (前看符号个数) 条 Item1，总数应当等于所有前看集大小之和
    let expected: usize = lookaheads.iter().flat_map(|m| m.values()).map(|s| s.len()).sum();
    let actual: usize = lalr.iter().map(|s| s.len()).sum();
    assert_eq!(actual, expected);

    // [n -> N ·] 有两个前看符号，所以摊成两条
    let state = state_with(&lookaheads, 3, 1);
    let expanded: Vec<u32> = lalr[state]
        .iter()
        .filter(|item| item.rule_index == 3 && item.position == 1)
        .map(|item| item.lookahead)
        .collect();
    assert_eq!(expanded.len(), 2, "{expanded:?}");
}

// ============ ACTION / GOTO 表 ============

#[test]
fn action_columns_are_terminals_and_goto_columns_are_nonterminals() {
    for (label, grammar) in catalogue() {
        let table = ParseTable::generate_parse_table(&grammar);
        for &(state, symbol) in table.action.keys() {
            assert!(
                grammar.terminals[symbol as usize],
                "{label}: ACTION 出现非终结符列，状态 {state} 符号 {}",
                grammar.names[symbol as usize]
            );
        }
        for &(state, symbol) in table.goto.keys() {
            assert!(
                !grammar.terminals[symbol as usize],
                "{label}: GOTO 出现终结符列，状态 {state} 符号 {}",
                grammar.names[symbol as usize]
            );
        }
    }
}

#[test]
fn shift_and_goto_partition_the_transitions() {
    // 移进和 goto 只取决于 LR(0) 转移，两者相加必须恰好等于转移总数：
    // 不多（没有凭空造出的动作）也不少（没有被归约覆盖掉的移进）
    for (label, grammar) in catalogue() {
        let (_, transitions) = build_kernels(&grammar);
        let table = ParseTable::generate_parse_table(&grammar);
        let (shift, _, _) = count_actions(&table);
        assert_eq!(
            shift + table.goto.len(),
            transitions.len(),
            "{label}: 移进 {shift} + goto {} != 转移 {}",
            table.goto.len(),
            transitions.len()
        );
    }
}

#[test]
fn accept_is_unique_and_on_the_end_symbol() {
    for (label, grammar) in catalogue() {
        let table = ParseTable::generate_parse_table(&grammar);
        let accepts: Vec<(u32, u32)> = table
            .action
            .iter()
            .filter(|(_, action)| matches!(action, Action::Accept))
            .map(|(&key, _)| key)
            .collect();
        assert_eq!(accepts.len(), 1, "{label}: 接受动作应当唯一，实际 {accepts:?}");
        assert_eq!(accepts[0].1, grammar.end_symbol(), "{label}: 接受动作必须在 $end 列");
    }
}

#[test]
fn every_state_has_something_to_do() {
    for (label, grammar) in catalogue() {
        let (states, _) = build_kernels(&grammar);
        let table = ParseTable::generate_parse_table(&grammar);
        for state in 0..states.len() as u32 {
            let has_action = table.action.keys().any(|&(s, _)| s == state);
            let has_goto = table.goto.keys().any(|&(s, _)| s == state);
            assert!(has_action || has_goto, "{label}: 状态 {state} 既无动作也无 goto");
        }
    }
}

#[test]
fn every_rule_except_the_augmented_one_can_be_reduced() {
    // 规则 0 走 Accept 而不是 Reduce，其余每条规则都必须在表里至少有一处归约，
    // 否则它永远不会被用到
    for (label, grammar) in catalogue() {
        let table = ParseTable::generate_parse_table(&grammar);
        let reduced: HashSet<u32> = table
            .action
            .values()
            .filter_map(|action| match action {
                Action::Reduce(rule) => Some(*rule),
                _ => None,
            })
            .collect();
        for rule in 1..grammar.rules.len() as u32 {
            assert!(
                reduced.contains(&rule),
                "{label}: 规则 {rule} [{}] 在表里没有任何归约动作",
                render_rule(&grammar, rule as usize)
            );
        }
    }
}

#[test]
fn epsilon_production_gets_a_reduce_action() {
    // [opt -> ·] 的 position 是 0，不在任何内核里，只在闭包里。
    // 建表时如果只扫内核项，这条归约会整条丢掉，`A B` 就永远解析不了
    let grammar = epsilon_reduce();
    let table = ParseTable::generate_parse_table(&grammar);
    let eps = grammar.rules.iter().position(|rule| rule.right.is_empty()).unwrap();
    assert_eq!(render_rule(&grammar, eps), "opt -> ε");

    let sites: Vec<(u32, &str)> = table
        .action
        .iter()
        .filter(|(_, action)| matches!(action, Action::Reduce(r) if *r as usize == eps))
        .map(|(&(state, symbol), _)| (state, grammar.names[symbol as usize].as_str()))
        .collect();
    // opt 可空，所以读完 A 之后若下一个是 B 就要先归约出一个空的 opt
    assert_eq!(sites.len(), 1, "{sites:?}");
    assert_eq!(sites[0].1, "B", "空 opt 只在前看 B 时归约");
}

#[test]
fn minimal_grammar_table_matches_hand_computation() {
    let grammar = minimal();
    let table = ParseTable::generate_parse_table(&grammar);
    let a = sym(&grammar, "A");
    let end = grammar.end_symbol();
    // 状态 0 读 A 移进到状态 1；状态 1 读 $end 接受。没有别的格子
    assert_eq!(table.action[&(0, a)], Action::Shift(1));
    assert_eq!(table.action[&(1, end)], Action::Accept);
    assert_eq!(table.action.len(), 2, "{:?}", table.action);
    assert!(table.goto.is_empty(), "没有非终结符转移");
}

#[test]
fn dangling_else_resolves_to_shift() {
    let grammar = dangling_else();
    let table = ParseTable::generate_parse_table(&grammar);

    // 全表恰好一处移进-归约冲突，就是教科书里那个
    assert_eq!(
        shift_reduce_conflicts(&grammar, &table),
        ["前看 ELSE 时丢掉归约 [stmt -> IF stmt]"]
    );

    // 冲突点上留下的必须是移进 —— else 绑到最近的 if
    let else_sym = sym(&grammar, "ELSE");
    let shifted: Vec<u32> = table
        .action
        .iter()
        .filter(|&(&(_, symbol), action)| symbol == else_sym && matches!(action, Action::Shift(_)))
        .map(|(&(state, _), _)| state)
        .collect();
    assert_eq!(shifted.len(), 1, "ELSE 列上应当恰好有一处移进：{shifted:?}");

    // 而且那个状态上 ELSE 不是归约
    assert!(
        !matches!(table.action[&(shifted[0], else_sym)], Action::Reduce(_)),
        "贪婪策略必须保留移进"
    );
}

#[test]
#[should_panic(expected = "reduce-reduce conflicts")]
fn reduce_reduce_conflict_panics() {
    // goto(I0, C) = {[a -> C ·], [b -> C ·]}，两者前看都是 {X}。
    // 这种冲突没有默认解，必须在编译期炸掉而不是静默挑一条
    ParseTable::generate_parse_table(&reduce_reduce());
}

// ============ 发射层 ============

#[test]
fn dense_cells_round_trip_every_entry() {
    for (label, grammar) in catalogue() {
        let table = ParseTable::generate_parse_table(&grammar);
        let symbol_count = grammar.names.len();
        let cells = table.dense_cells();
        let (states, _) = build_kernels(&grammar);
        assert_eq!(cells.len(), states.len() * symbol_count, "{label}: 稠密表尺寸");

        for (&(state, symbol), &action) in &table.action {
            let cell = cells[state as usize * symbol_count + symbol as usize];
            let decoded = match cell {
                CELL_ERROR => panic!(
                    "{label}: 状态 {state} 符号 {} 的动作丢了",
                    grammar.names[symbol as usize]
                ),
                CELL_ACCEPT => Action::Accept,
                v if v > 0 => Action::Shift((v - 1) as u32),
                v => Action::Reduce((-v - 1) as u32),
            };
            assert_eq!(decoded, action, "{label}: 状态 {state} 符号 {}", grammar.names[symbol as usize]);
        }
        for (&(state, symbol), &target) in &table.goto {
            let cell = cells[state as usize * symbol_count + symbol as usize];
            assert!(cell > 0, "{label}: goto 格子必须是正数，实际 {cell}");
            assert_eq!((cell - 1) as u32, target, "{label}");
        }
        // 非零格子数 = 两张表条目数之和，说明 ACTION 和 GOTO 的列没有互相覆盖
        let filled = cells.iter().filter(|&&c| c != CELL_ERROR).count();
        assert_eq!(
            filled,
            table.action.len() + table.goto.len(),
            "{label}: ACTION 和 GOTO 的列不该重叠"
        );
    }
}

#[test]
fn reduce_cells_never_collide_with_shift_encoding() {
    // 编码依赖「+1 让 0 空出来表示无动作」以及 i32::MAX 专属 accept。
    // 状态号和规则下标都必须留在安全区间内，否则解码会串
    for (label, grammar) in catalogue() {
        let table = ParseTable::generate_parse_table(&grammar);
        let (states, _) = build_kernels(&grammar);
        let bound = states.len().max(grammar.rules.len());
        for &cell in &table.dense_cells() {
            assert_ne!(cell, i32::MIN, "{label}: i32::MIN 取负会溢出");
            if cell != CELL_ERROR && cell != CELL_ACCEPT {
                let magnitude = cell.unsigned_abs() - 1;
                assert!(
                    (magnitude as usize) < bound,
                    "{label}: 格子 {cell} 解出的下标 {magnitude} 越界（上界 {bound}）"
                );
            }
        }
    }
}

#[test]
fn emitted_rule_tables_let_the_driver_reduce() {
    // 归约只靠这两张小表：弹 RULE_RHS_LEN[r] 帧，再用 RULE_LHS[r] 查 GOTO。
    // 它们必须逐条对上文法，否则解析器会弹错栈深度
    for (label, grammar) in catalogue() {
        let table = ParseTable::generate_parse_table(&grammar);
        assert_eq!(table.rule_lhs.len(), grammar.rules.len(), "{label}");
        assert_eq!(table.rule_rhs_len.len(), grammar.rules.len(), "{label}");
        for (i, rule) in grammar.rules.iter().enumerate() {
            assert_eq!(table.rule_lhs[i], rule.left, "{label}: 规则 {i} 左部");
            assert_eq!(
                table.rule_rhs_len[i] as usize,
                rule.right.len(),
                "{label}: 规则 {i} 右部长度"
            );
        }
    }
}

#[test]
fn emitted_tokens_contain_the_expected_api() {
    let grammar = minimal();
    let table = ParseTable::generate_parse_table(&grammar);
    let emitted = table.export().to_string();

    // 规模常量必须和分析结果一致
    assert!(emitted.contains("NUM_STATES : usize = 2usize"), "缺 NUM_STATES");
    assert!(emitted.contains("NUM_SYMBOLS : usize = 3usize"), "缺 NUM_SYMBOLS");
    assert!(emitted.contains("NUM_RULES : usize = 1usize"), "缺 NUM_RULES");
    assert!(
        emitted.contains(&format!("END_SYMBOL : u32 = {}u32", grammar.end_symbol())),
        "缺 END_SYMBOL"
    );

    // 消费方要用的 API 一个都不能少
    for item in [
        "SYMBOL_NAMES",
        "IS_TERMINAL",
        "RULE_LHS",
        "RULE_RHS_LEN",
        "static TABLE",
        "pub enum Action",
        "pub fn action",
        "pub fn goto",
        "pub fn symbol_index",
        "pub fn expected_terminals",
    ] {
        assert!(emitted.contains(item), "发射结果里缺 {item}");
    }

    // 发射的是 token 而不是字符串，所以名字必须是带引号的字面量
    assert!(emitted.contains(r#""$end""#), "SYMBOL_NAMES 里应有 \"$end\" 字面量");
    assert!(emitted.contains(r#""start""#));
}
