use proc_macro::TokenStream;
mod generator;

#[proc_macro]
pub fn grammar(input: TokenStream) -> TokenStream {
    // let root = generator::generate_grammar_tree(input);


    grammar_impl(input.into()).into()
}

fn grammar_impl(input: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    eprintln!("grammar! input = {input}");
    for tt in input.clone() {
        eprintln!("  tt = {tt:?}");
    }
    input //暂时原样回吐；TODO: parse DSL -> build table -> emit static tables
}

#[cfg(test)]
mod tests {
    use super::generator::Grammar;
    use quote::quote;
    use std::collections::HashSet;

    /// 真实文法，所有测例共用
    fn lua_grammar() -> Grammar {
        //quote!构造TokenStream最方便；也可用 "…".parse::<proc_macro2::TokenStream>()
        let input = quote! {

            //这里已经算增广文法了
            chunk
                : block
                ;
            
            block
                : stat_list
                | stat_list retstat
                | retstat
                | /* empty */
                ;
            
            stat_list
                : stat
                | stat_list stat
                ;
            
            stat
                : ';'                            /* empty stmt dropped */
                | varlist '=' explist
                | prefixexp
                | label
                | BREAK
                | GOTO NAME
                | DO block END
                | WHILE exp DO block END
                | REPEAT block UNTIL exp
                | IF exp THEN block elseif_list END
            
                | IF exp THEN block elseif_list ELSE block END
            
                | FOR NAME '=' exp ',' exp DO block END
            
                | FOR NAME '=' exp ',' exp ',' exp DO block END
            
                | FOR namelist IN explist DO block END
            
                | FUNCTION funcname funcbody
                | LOCAL FUNCTION NAME funcbody
                | LOCAL decllist
                | LOCAL decllist '=' explist
                | CLASS NAME classbody
                | CLASS NAME EXTENDS NAME classbody
            
                ;
            
            elseif_list
                : /* empty */
                | elseif_list ELSEIF exp THEN block
            
                ;
            
            retstat
                : RETURN
                | RETURN ';'
                | RETURN explist
                | RETURN explist ';'
                ;
            
            label
                : DBCOLON NAME DBCOLON
                ;
            
            funcname
                : dotted_name
                | dotted_name ':' NAME
                ;
            
            dotted_name
                : NAME
                | dotted_name '.' NAME
                ;
            
            varlist
                : var
                | varlist ',' var
                ;
            
            var
                : prefixexp
                ;
            
            namelist
                : NAME
                | namelist ',' NAME
                ;
            
            decllist
                : NAME optype
                | decllist ',' NAME optype
                ;
            
            optype
                : /* empty */
                | ':' type
                ;
            
            type
                : uniontype
                | functype
                ;
            
            uniontype
                : basictype
                | uniontype '|' basictype
                ;
            
            basictype
                : NAME
                | NIL                            /* nil type */
                | NAME '<' typeargs '>'
            
                | '(' type ')'                   /* grouping: (function()->A)|B */
                ;
            
            typeargs
                : type
                | typeargs ',' type
                ;
            
            functype
                : FUNCTION '(' ')'
                | FUNCTION '(' ')' rettype
                | FUNCTION '(' parlist ')'
                | FUNCTION '(' parlist ')' rettype
            
                ;
            
            rettype
                : ARROW retspec
                ;
            
            retspec
                : type                           /* single return */
                | '(' ')'                        /* void */
                | '(' multilist ')'
                ;

            multilist                            /* excludes the single non-vararg case:
                                                    that goes through basictype grouping */
                : typelist ',' rtype             /* two or more */
                | ELLIPSIS                       /* -> (...) */
                | ELLIPSIS type                  /* -> (...any) */
                ;
            
            typelist
                : rtype
                | typelist ',' rtype
                ;
            
            rtype
                : type
                | ELLIPSIS type
                | ELLIPSIS
                ;
            
            classbody
                : '{' '}'
                | '{' classfields '}'
                ;
            
            classfields
                : classfield
                | classfields ',' classfield
                ;
            
            classfield
                : NAME ':' type
                | NAME ':' type '=' exp
                | NAME '=' exp
                | methodsig
                | methodsig block END
                | methodsig '=' exp
                ;
            
            methodsig
                : NAME '(' ')'
                | NAME '(' ')' rettype
                | NAME '(' parlist ')'
                | NAME '(' parlist ')' rettype
                ;
            
            explist
                : exp
                | explist ',' exp
                ;
            
            /* Precedence and associativity are encoded in the rules themselves:
               one nonterminal per precedence level, left recursion for left
               associativity, right recursion for right associativity. */

            exp
                : or_exp
                ;

            or_exp
                : or_exp OR and_exp
                | and_exp
                ;

            and_exp
                : and_exp AND cmp_exp
                | cmp_exp
                ;

            cmp_exp
                : cmp_exp '<' bor_exp
                | cmp_exp '>' bor_exp
                | cmp_exp LE bor_exp
                | cmp_exp GE bor_exp
                | cmp_exp EQ bor_exp
                | cmp_exp NE bor_exp
                | bor_exp
                ;

            bor_exp
                : bor_exp '|' bxor_exp
                | bxor_exp
                ;

            bxor_exp
                : bxor_exp '~' band_exp
                | band_exp
                ;

            band_exp
                : band_exp '&' sh_exp
                | sh_exp
                ;

            sh_exp
                : sh_exp SHL cat_exp
                | sh_exp SHR cat_exp
                | cat_exp
                ;

            cat_exp                              /* '..' is right associative */
                : add_exp CONCAT cat_exp
                | add_exp
                ;

            add_exp
                : add_exp '+' mul_exp
                | add_exp '-' mul_exp
                | mul_exp
                ;

            mul_exp
                : mul_exp '*' un_exp
                | mul_exp '/' un_exp
                | mul_exp IDIV un_exp
                | mul_exp '%' un_exp
                | un_exp
                ;

            un_exp                               /* unary prefix operators */
                : NOT un_exp
                | '#' un_exp
                | '-' un_exp
                | '~' un_exp
                | pow_exp
                ;

            pow_exp                              /* '^' outranks unary, yet its right
                                                    operand may be unary: 2^-3 */
                : atom '^' un_exp
                | atom
                ;

            atom
                : NIL
                | TRUE
                | FALSE
                | NUMERAL
                | STRING
                | ELLIPSIS
                | functiondef
                | prefixexp
                | tableconstructor
                ;
            
            prefixexp
                : NAME
                | '(' exp ')'
                | prefixexp '[' exp ']'
                | prefixexp '.' NAME
                | prefixexp args
                | prefixexp ':' NAME args
                ;
            
            args
                : '(' ')'
                | '(' explist ')'
                | tableconstructor
                | STRING
                ;
            
            functiondef
                : FUNCTION funcbody
                ;
            
            funcbody
                : '(' ')' block END
                | '(' ')' rettype block END
                | '(' parlist ')' block END
                | '(' parlist ')' rettype block END
                ;
            
            parlist
                : params
                | params ',' vararg
                | vararg
                ;
            
            params
                : param
                | params ',' param
                ;
            
            param
                : NAME optype
                ;
            
            vararg
                : ELLIPSIS
                | ELLIPSIS type
                ;
            
            tableconstructor
                : '{' '}'
                | '{' fieldlist '}'
                ;
            
            fieldlist
                : fields
                | fields fieldsep
                ;
            
            fields
                : field
                | fields fieldsep field
                ;
            
            field
                : '[' exp ']' '=' exp
                | NAME '=' exp
                | exp
                ;
            
            fieldsep
                : ','
                | ';'
                ;
        };
        Grammar::build_grammar(input)
    }

    fn sym(grammar: &Grammar, name: &str) -> u32 {
        grammar
            .names
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("symbol {name} not in grammar")) as u32
    }

    /// FIRST 集里的下标翻回名字，-1 渲染成 "ε"，排序后便于断言
    fn first_of(grammar: &Grammar, name: &str) -> Vec<String> {
        let mut names: Vec<String> = grammar.firsts[sym(grammar, name) as usize]
            .iter()
            .map(|&f| render(grammar, f))
            .collect();
        names.sort();
        names
    }

    fn render(grammar: &Grammar, f: i32) -> String {
        if f == -1 {
            "ε".to_string()
        } else {
            grammar.names[f as usize].clone()
        }
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

    /// FIRST(block) 去掉 ε：block 是文法里最常出现在可空位置的符号
    const FIRST_BLOCK: [&str; 15] = [
        "'('", "';'", "BREAK", "CLASS", "DBCOLON", "DO", "FOR", "FUNCTION", "GOTO", "IF", "LOCAL",
        "NAME", "REPEAT", "RETURN", "WHILE",
    ];

    #[test]
    fn print_input_stream() {
        println!("parsed grammar:\n{}", lua_grammar());
    }

    // ---------- build_first_set ----------

    #[test]
    fn exactly_four_symbols_are_nullable() {
        let grammar = lua_grammar();
        let mut nullable: Vec<&str> = grammar
            .firsts
            .iter()
            .enumerate()
            .filter(|(_, first)| first.contains(&-1))
            .map(|(i, _)| grammar.names[i].as_str())
            .collect();
        nullable.sort();
        // block / elseif_list / optype 写了 ε 候选式；chunk -> block 由传播得到
        assert_eq!(nullable, ["block", "chunk", "elseif_list", "optype"]);
    }

    #[test]
    fn first_of_nullable_symbols() {
        let grammar = lua_grammar();
        // elseif_list -> ε | elseif_list ELSEIF exp THEN block
        // 可空符号出现在自己左递归的最左端：只看右部首符号的实现会漏掉 ELSEIF
        assert_eq!(first_of(&grammar, "elseif_list"), ["ELSEIF", "ε"]);
        // optype -> ε | ':' type
        assert_eq!(first_of(&grammar, "optype"), ["':'", "ε"]);
    }

    #[test]
    fn first_of_block_and_chunk() {
        let grammar = lua_grammar();
        let expected: Vec<String> = FIRST_BLOCK
            .iter()
            .map(|s| s.to_string())
            .chain(["ε".to_string()])
            .collect();
        assert_eq!(first_of(&grammar, "block"), expected);
        // chunk -> block 是唯一产生式，两者 FIRST 必须完全一致
        assert_eq!(first_of(&grammar, "chunk"), expected);
    }

    #[test]
    fn stat_is_block_without_return_and_not_nullable() {
        let grammar = lua_grammar();
        // block -> stat_list | stat_list retstat | retstat | ε
        // 所以 FIRST(block) 恰好是 FIRST(stat) 加上 RETURN 和 ε
        let expected: Vec<&str> = FIRST_BLOCK.iter().copied().filter(|s| *s != "RETURN").collect();
        assert_eq!(first_of(&grammar, "stat"), expected);
    }

    #[test]
    fn terminal_first_is_itself_and_never_nullable() {
        let grammar = lua_grammar();
        for (i, &is_terminal) in grammar.terminals.iter().enumerate() {
            if is_terminal {
                assert_eq!(
                    grammar.firsts[i],
                    HashSet::from([i as i32]),
                    "终结符 {} 的 FIRST 应当只有自己",
                    grammar.names[i]
                );
            }
        }
    }

    #[test]
    fn first_never_contains_a_nonterminal() {
        let grammar = lua_grammar();
        for (i, first) in grammar.firsts.iter().enumerate() {
            for &f in first {
                assert!(
                    f == -1 || grammar.terminals[f as usize],
                    "FIRST({}) 混入了非终结符 {}",
                    grammar.names[i],
                    grammar.names[f as usize]
                );
            }
        }
    }

    // ---------- get_seq_first ----------

    #[test]
    fn seq_first_crosses_two_consecutive_nullable_symbols() {
        let grammar = lua_grammar();
        // stat -> IF exp THEN block elseif_list END
        // THEN 之后的后缀 block elseif_list END：两个可空符号连续，END 必须可见
        let mut expected: Vec<String> = FIRST_BLOCK.iter().map(|s| s.to_string()).collect();
        expected.push("ELSEIF".to_string());
        expected.push("END".to_string());
        expected.sort();
        // END 不可空 ⇒ 继承的前看符号 UNTIL 不并入
        assert_eq!(
            seq_first_of(&grammar, &["block", "elseif_list", "END"], "UNTIL"),
            expected
        );
    }

    #[test]
    fn seq_first_folds_in_lookahead_when_suffix_is_fully_nullable() {
        let grammar = lua_grammar();
        // 后缀 block elseif_list 整体可空 ⇒ 必须并入继承来的前看符号
        let mut expected: Vec<String> = FIRST_BLOCK.iter().map(|s| s.to_string()).collect();
        expected.push("ELSEIF".to_string());
        expected.push("UNTIL".to_string());
        expected.sort();
        assert_eq!(
            seq_first_of(&grammar, &["block", "elseif_list"], "UNTIL"),
            expected
        );
    }

    #[test]
    fn seq_first_of_empty_suffix_is_just_the_lookahead() {
        let grammar = lua_grammar();
        // 点走到产生式末尾时后缀为空：FIRST(ε · a) = {a}
        assert_eq!(seq_first_of(&grammar, &[], "END"), ["END"]);
    }

    #[test]
    fn seq_first_stops_at_the_first_non_nullable_symbol() {
        let grammar = lua_grammar();
        // stat -> REPEAT block UNTIL exp：block 可空，UNTIL 不可空 ⇒ exp 看不到
        let mut expected: Vec<String> = FIRST_BLOCK.iter().map(|s| s.to_string()).collect();
        expected.push("UNTIL".to_string());
        expected.sort();
        assert_eq!(
            seq_first_of(&grammar, &["block", "UNTIL", "exp"], "END"),
            expected
        );
    }

    #[test]
    fn seq_first_dedups_a_lookahead_already_in_the_suffix() {
        let grammar = lua_grammar();
        // block 可空且 NAME ∈ FIRST(block)，前看符号也是 NAME：
        // 结果里 NAME 只能有一份，否则前看集无法用 == 比较（LALR 合并状态依赖这一点）
        let seq = [sym(&grammar, "block")];
        let result = grammar.get_seq_first(&seq, sym(&grammar, "NAME"));
        assert_eq!(result.len(), FIRST_BLOCK.len());
        assert!(result.contains(&sym(&grammar, "NAME")));
    }
}
