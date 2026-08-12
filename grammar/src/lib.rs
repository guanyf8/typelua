use proc_macro::TokenStream;
mod generator;
use generator::{Grammar, generate_grammar_tree};

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
    use super::*;
    use quote::quote;

    #[test]
    fn print_input_stream() {
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
        let output = generate_grammar_tree(input);
        println!("parsed grammar:\n{output}");

    }
}
