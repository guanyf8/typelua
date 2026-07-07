# typelua

## sample
```lua
-- ========================= Classes =========================
class A{
    a:string="hello",
    b:boolean,
    c:number,
    greet:function(msg:string) -> string,       -- 成员函数【声明】：只给函数类型签名，没有函数体
    greet2(msg:string) -> number ,
    hello = function() -> string return "hello" end,   -- 成员函数【定义·形式1】：类体内直接赋一个匿名函数(function 不省略)
    bye() -> string return "bye" end,       -- 成员函数【声明+定义】：
    hello_again()->(string,number) return "hello",1 end
}

class B extends A{
    d:number
}

-- 成员函数【定义·形式2】：类体外用具名方法定义(function 不可省略)
function A:greet(msg:string) -> string
    return msg
end

-- ========================= Typed locals =========================
local a:string="hello"
local a:string,b:number,c:number="world",1,2
local a="hello"          -- 可以直接判断类型的不需要类型声明
-- local a               -- 不允许：不初始化下必须要声明类型(见 tests/tlua_bad/01)

-- ========================= Functions =========================
-- 具名局部函数(function 不可省略)，参数带类型、含函数类型参数，单返回值
local function a(a:string, b:number, c:function(msg:string)) -> string
    return "hahaha"
end

-- 匿名函数赋值给变量(function 不省略)，函数类型参数带返回值，多返回值
local b = function(a:string, b:number, c:function(msg:string) -> boolean) -> (number, string)
    return 1, "x"
end

-- 具名全局函数：无返回值(void)
function noop() -> ()
end

-- 变长返回
function unpackall(t) -> (...any)
    return 1, 2, 3
end

local t:table<string,any>

class MyType{
    log(...string),
    system_os()->number,
    os:function()->string
}

-- 是一个C层注册的全局变量
local _mytype:MyType

local E:A|B|nil
```


## typelua grammar
约定：{A} 表示 0 个或多个 A；[A] 表示可选 A。
标记：  (=Lua)  与 Lua 原定义完全相同
        (~Lua)  在 Lua 基础上修改
        (+TLUA) TypeLua 新增

--------------------------------------------------
```
chunk ::= block                                                        (=Lua)

block ::= {stat} [retstat]                                             (=Lua)

stat ::=  ‘;’ |                                                        (~Lua)
     varlist ‘=’ explist |
     functioncall |
     label |
     break |
     goto Name |
     do block end |
     while exp do block end |
     repeat block until exp |
     if exp then block {elseif exp then block} [else block] end |
     for Name ‘=’ exp ‘,’ exp [‘,’ exp] do block end |
     for namelist in explist do block end |
     function funcname funcbody |
     local function Name funcbody |
     local decllist [‘=’ explist] |            -- (~Lua) namelist -> decllist
     class Name [extends Name] classbody        -- (+TLUA) 类声明

retstat ::= return [explist] [‘;’]                                     (=Lua)

label ::= ‘::’ Name ‘::’                                               (=Lua)

funcname ::= Name {‘.’ Name} [‘:’ Name]                               (=Lua)

varlist ::= var {‘,’ var}                                              (=Lua)

var ::=  Name | prefixexp ‘[’ exp ‘]’ | prefixexp ‘.’ Name            (=Lua)

namelist ::= Name {‘,’ Name}                                          (=Lua)

explist ::= exp {‘,’ exp}                                              (=Lua)

exp ::=  nil | false | true | Numeral | LiteralString | ‘...’ |        (=Lua)
     functiondef | prefixexp | tableconstructor |
     exp binop exp | unop exp

prefixexp ::= var | functioncall | ‘(’ exp ‘)’                        (=Lua)

functioncall ::=  prefixexp args | prefixexp ‘:’ Name args            (=Lua)

args ::=  ‘(’ [explist] ‘)’ | tableconstructor | LiteralString        (=Lua)

functiondef ::= function funcbody                                      (=Lua)

funcbody ::= ‘(’ [parlist] ‘)’ [rettype] block end     -- (~Lua) 加 [rettype]

parlist ::= param {‘,’ param} [‘,’ vararg] | vararg    -- (~Lua) 参数可带类型
param   ::= Name [‘:’ type]                             -- (+TLUA)
vararg  ::= ‘...’ [type]                                -- (~Lua) 变长可带类型

tableconstructor ::= ‘{’ [fieldlist] ‘}’                              (=Lua)
fieldlist ::= field {fieldsep field} [fieldsep]                       (=Lua)
field ::= ‘[’ exp ‘]’ ‘=’ exp | Name ‘=’ exp | exp                    (=Lua)
fieldsep ::= ‘,’ | ‘;’                                                (=Lua)

binop ::=  ‘+’ | ‘-’ | ‘*’ | ‘/’ | ‘//’ | ‘^’ | ‘%’ |                 (=Lua)
     ‘&’ | ‘~’ | ‘|’ | ‘>>’ | ‘<<’ | ‘..’ |
     ‘<’ | ‘<=’ | ‘>’ | ‘>=’ | ‘==’ | ‘~=’ | and | or

unop ::= ‘-’ | not | ‘#’ | ‘~’                                        (=Lua)

======================= 以下全部 (+TLUA) =======================

decllist ::= Name [‘:’ type] {‘,’ Name [‘:’ type]}
     -- 语义约束：`local decllist` 若没有 `= explist`（无初始化），
     --           则每个 Name 都必须带 `: type`。

-- ---- 类型系统 ----
type      ::= Name
            | nil                            -- nil 类型（用于可空/联合）
            | Name ‘<’ typeargs ‘>’          -- 泛型（支持任意嵌套）
            | functype
            | type ‘|’ type                   -- 联合类型 A|B|nil（左结合、扁平化）
typeargs  ::= type {‘,’ type}
functype  ::= function ‘(’ [parlist] ‘)’ [rettype]

rettype   ::= ‘->’ retspec
retspec   ::= type                            -- 单返回值
            | ‘(’ [typelist] ‘)’             -- 0 个(void) / 多返回值
typelist  ::= rtype {‘,’ rtype}
rtype     ::= type | ‘...’ [type]             -- 变长返回

-- ---- 类 ----
classbody  ::= ‘{’ [classfield {‘,’ classfield}] ‘}’
classfield ::= Name ‘:’ type [‘=’ exp]        -- 属性：声明[+默认值]
             | Name ‘=’ exp                    -- 属性：定义(类型推断)
             | methodsig [ block end | ‘=’ exp ]
                                               -- 方法：签名 / 内联定义 / 赋值默认
methodsig  ::= Name ‘(’ [parlist] ‘)’ [rettype]
```

## flex
```flex
[ \t\r]+            { /* whitespace */ }
\n                  { /* newline; yylineno tracked by option */ }

"--"{LONGOPEN}      { if (read_long_bracket(bracket_level(yytext+2), NULL) < 0) {
                          fprintf(stderr, "line %d: unfinished long comment\n", yylineno);
                          return 0;
                      } }
"--"[^\n]*          { /* short comment */ }

{LONGOPEN}          { char *body = NULL;
                      if (read_long_bracket(bracket_level(yytext), &body) < 0) {
                          fprintf(stderr, "line %d: unfinished long string\n", yylineno);
                          return 0;
                      }
                      yylval.str = body ? body : strdup("");
                      return STRING; }

\"(\\(.|\n)|[^"\\\n])*\"   { yylval.str = strdup(yytext); return STRING; }
'(\\(.|\n)|[^'\\\n])*'     { yylval.str = strdup(yytext); return STRING; }

0[xX][0-9a-fA-F]*\.?[0-9a-fA-F]*([pP][+-]?[0-9]+)?   { yylval.str = strdup(yytext); return NUMERAL; }
[0-9]+\.?[0-9]*([eE][+-]?[0-9]+)?                    { yylval.str = strdup(yytext); return NUMERAL; }
\.[0-9]+([eE][+-]?[0-9]+)?                           { yylval.str = strdup(yytext); return NUMERAL; }

"class"     { return CLASS; }      /* TLUA: class keyword */
"extends"   { return EXTENDS; }    /* TLUA: inheritance keyword         */
"and"       { return AND; }
"break"     { return BREAK; }
"do"        { return DO; }
"else"      { return ELSE; }
"elseif"    { return ELSEIF; }
"end"       { return END; }
"false"     { return FALSE; }
"for"       { return FOR; }
"function"  { return FUNCTION; }
"goto"      { return GOTO; }
"if"        { return IF; }
"in"        { return IN; }
"local"     { return LOCAL; }
"nil"       { return NIL; }
"not"       { return NOT; }
"or"        { return OR; }
"repeat"    { return REPEAT; }
"return"    { return RETURN; }
"then"      { return THEN; }
"true"      { return TRUE; }
"until"     { return UNTIL; }
"while"     { return WHILE; }

[A-Za-z_][A-Za-z0-9_]*   { yylval.str = strdup(yytext); return NAME; }

"..."       { return ELLIPSIS; }
".."        { return CONCAT; }
"->"        { return ARROW; }      /* TLUA: return-type arrow */
"::"        { return DBCOLON; }
"=="        { return EQ; }
"~="        { return NE; }
"<="        { return LE; }
">="        { return GE; }
"<<"        { return SHL; }
">>"        { if (SHL_depth > 0) { yyless(1); return '>'; }   /* TLUA: split nested generic close */
              return SHR; }
"//"        { return IDIV; }

[-+*/%^#&~|<>=(){}\[\];:,.]   { return yytext[0]; }

.           { fprintf(stderr, "line %d: unexpected character '%s'\n", yylineno, yytext);
              return 0; }
```

## bison
```bison
chunk
    : block        
    ;

block
    : stat_list             { $$ = $1; }
    | stat_list retstat     { $$ = $1; ast_add($$, $2); }
    | retstat               { $$ = ast_new("Block"); ast_add($$, $1); }
    | /* empty */           { $$ = ast_new("Block"); }
    ;

stat_list
    : stat                  { $$ = ast_new("Block"); ast_add($$, $1); }
    | stat_list stat        { $$ = $1; ast_add($$, $2); }
    ;

stat
    : ';'                            { $$ = NULL; }   /* empty stmt dropped */
    | varlist '=' explist            { $$ = ast_new("Assign");
                                       ast_add($$, $1); ast_add($$, $3); }
    | prefixexp                      { if ($1->aux != PK_CALL) {
                                           yyerror("syntax error (statement is not a function call)");
                                           YYERROR;
                                       }
                                       $$ = $1; }
    | label                          { $$ = $1; }
    | BREAK                          { $$ = ast_new("Break"); }
    | GOTO NAME                      { $$ = ast_own("Goto", $2); }
    | DO block END                   { $$ = ast_new("Do"); ast_add($$, $2); }
    | WHILE exp DO block END         { $$ = ast_new("While");
                                       ast_add($$, $2); ast_add($$, $4); }
    | REPEAT block UNTIL exp         { $$ = ast_new("Repeat");
                                       ast_add($$, $2); ast_add($$, $4); }
    | IF exp THEN block elseif_list END
                                     { $$ = ast_new("If");
                                       ast_add($$, $2); ast_add($$, $4);
                                       ast_merge($$, $5); }
    | IF exp THEN block elseif_list ELSE block END
                                     { $$ = ast_new("If");
                                       ast_add($$, $2); ast_add($$, $4);
                                       ast_merge($$, $5);
                                       Node *e = ast_new("Else");
                                       ast_add(e, $7); ast_add($$, e); }
    | FOR NAME '=' exp ',' exp DO block END
                                     { $$ = ast_own("ForNum", $2);
                                       ast_add($$, $4); ast_add($$, $6);
                                       ast_add($$, $8); }
    | FOR NAME '=' exp ',' exp ',' exp DO block END
                                     { $$ = ast_own("ForNum", $2);
                                       ast_add($$, $4); ast_add($$, $6);
                                       Node *st = ast_new("Step"); ast_add(st, $8);
                                       ast_add($$, st); ast_add($$, $10); }
    | FOR namelist IN explist DO block END
                                     { $$ = ast_new("ForIn");
                                       ast_add($$, $2); ast_add($$, $4);
                                       ast_add($$, $6); }
    | FUNCTION funcname funcbody     { $$ = ast_new("Function");
                                       ast_add($$, $2); ast_add($$, $3); }
    | LOCAL FUNCTION NAME funcbody   { $$ = ast_own("LocalFunction", $3);
                                       ast_add($$, $4); }
    | LOCAL namelist                 { $$ = ast_new("Local"); ast_add($$, $2); }
    | LOCAL namelist '=' explist     { $$ = ast_new("Local");
                                       ast_add($$, $2); ast_add($$, $4); }
    ;

elseif_list
    : /* empty */                    { $$ = ast_new("ElseIfChain"); }
    | elseif_list ELSEIF exp THEN block
                                     { $$ = $1;
                                       Node *e = ast_new("ElseIf");
                                       ast_add(e, $3); ast_add(e, $5);
                                       ast_add($$, e); }
    ;

retstat
    : RETURN                         { $$ = ast_new("Return"); }
    | RETURN ';'                     { $$ = ast_new("Return"); }
    | RETURN explist                 { $$ = ast_new("Return"); ast_add($$, $2); }
    | RETURN explist ';'             { $$ = ast_new("Return"); ast_add($$, $2); }
    ;

label
    : DBCOLON NAME DBCOLON           { $$ = ast_own("Label", $2); }
    ;

funcname
    : dotted_name                    { $$ = ast_own("FuncName", $1); }
    | dotted_name ':' NAME           { $$ = ast_own("FuncName",
                                           sconcat($1, ":", $3));
                                       free($1); free($3); }
    ;

dotted_name
    : NAME                           { $$ = $1; }
    | dotted_name '.' NAME           { $$ = sconcat($1, ".", $3);
                                       free($1); free($3); }
    ;

varlist
    : var                            { $$ = ast_new("VarList"); ast_add($$, $1); }
    | varlist ',' var                { $$ = $1; ast_add($$, $3); }
    ;

var
    : prefixexp                      { if ($1->aux == PK_CALL || $1->aux == PK_PAREN) {
                                           yyerror("cannot assign to this expression (not a variable)");
                                           YYERROR;
                                       }
                                       $$ = $1; }
    ;

namelist
    : NAME                           { $$ = ast_new("NameList");
                                       ast_add($$, ast_own("Name", $1)); }
    | namelist ',' NAME              { $$ = $1;
                                       ast_add($$, ast_own("Name", $3)); }
    ;

explist
    : exp                            { $$ = ast_new("ExpList"); ast_add($$, $1); }
    | explist ',' exp                { $$ = $1; ast_add($$, $3); }
    ;

exp
    : NIL                            { $$ = ast_new("Nil"); }
    | TRUE                           { $$ = ast_new("True"); }
    | FALSE                          { $$ = ast_new("False"); }
    | NUMERAL                        { $$ = ast_own("Number", $1); }
    | STRING                         { $$ = ast_own("String", $1); }
    | ELLIPSIS                       { $$ = ast_new("Vararg"); }
    | functiondef                    { $$ = $1; }
    | prefixexp                      { $$ = $1; }
    | tableconstructor               { $$ = $1; }
    | exp '+' exp                    { $$ = ast_binop("+",  $1, $3); }
    | exp '-' exp                    { $$ = ast_binop("-",  $1, $3); }
    | exp '*' exp                    { $$ = ast_binop("*",  $1, $3); }
    | exp '/' exp                    { $$ = ast_binop("/",  $1, $3); }
    | exp IDIV exp                   { $$ = ast_binop("//", $1, $3); }
    | exp '^' exp                    { $$ = ast_binop("^",  $1, $3); }
    | exp '%' exp                    { $$ = ast_binop("%",  $1, $3); }
    | exp '&' exp                    { $$ = ast_binop("&",  $1, $3); }
    | exp '~' exp                    { $$ = ast_binop("~",  $1, $3); }
    | exp '|' exp                    { $$ = ast_binop("|",  $1, $3); }
    | exp SHR exp                    { $$ = ast_binop(">>", $1, $3); }
    | exp SHL exp                    { $$ = ast_binop("<<", $1, $3); }
    | exp CONCAT exp                 { $$ = ast_binop("..", $1, $3); }
    | exp '<' exp                    { $$ = ast_binop("<",  $1, $3); }
    | exp LE exp                     { $$ = ast_binop("<=", $1, $3); }
    | exp '>' exp                    { $$ = ast_binop(">",  $1, $3); }
    | exp GE exp                     { $$ = ast_binop(">=", $1, $3); }
    | exp EQ exp                     { $$ = ast_binop("==", $1, $3); }
    | exp NE exp                     { $$ = ast_binop("~=", $1, $3); }
    | exp AND exp                    { $$ = ast_binop("and", $1, $3); }
    | exp OR exp                     { $$ = ast_binop("or",  $1, $3); }
    | '-' exp %prec UNARY            { $$ = ast_unop("-",   $2); }
    | NOT exp %prec UNARY            { $$ = ast_unop("not", $2); }
    | '#' exp %prec UNARY            { $$ = ast_unop("#",   $2); }
    | '~' exp %prec UNARY            { $$ = ast_unop("~",   $2); }
    ;

prefixexp
    : NAME                           { $$ = ast_own("Name", $1); $$->aux = PK_VAR; }
    | '(' exp ')'                    { $$ = ast_new("Paren"); ast_add($$, $2);
                                       $$->aux = PK_PAREN; }
    | prefixexp '[' exp ']'          { $$ = ast_new("Index");
                                       ast_add($$, $1); ast_add($$, $3);
                                       $$->aux = PK_VAR; }
    | prefixexp '.' NAME             { $$ = ast_own("Field", $3);
                                       ast_add($$, $1); $$->aux = PK_VAR; }
    | prefixexp args                 { $$ = ast_new("Call");
                                       ast_add($$, $1); ast_add($$, $2);
                                       $$->aux = PK_CALL; }
    | prefixexp ':' NAME args        { $$ = ast_own("MethodCall", $3);
                                       ast_add($$, $1); ast_add($$, $4);
                                       $$->aux = PK_CALL; }
    ;

args
    : '(' ')'                        { $$ = ast_new("Args"); }
    | '(' explist ')'                { $$ = ast_new("Args"); ast_merge($$, $2); }
    | tableconstructor               { $$ = ast_new("Args"); ast_add($$, $1); }
    | STRING                         { $$ = ast_new("Args");
                                       ast_add($$, ast_own("String", $1)); }
    ;

functiondef
    : FUNCTION funcbody              { $$ = ast_new("FunctionDef");
                                       ast_add($$, $2); }
    ;

funcbody
    : '(' ')' block END              { $$ = ast_new("FuncBody"); ast_add($$, $3); }
    | '(' parlist ')' block END      { $$ = ast_new("FuncBody");
                                       ast_add($$, $2); ast_add($$, $4); }
    ;

parlist
    : namelist                       { $$ = ast_new("ParList"); ast_merge($$, $1); }
    | namelist ',' ELLIPSIS          { $$ = ast_new("ParList"); ast_merge($$, $1);
                                       ast_add($$, ast_new("Vararg")); }
    | ELLIPSIS                       { $$ = ast_new("ParList");
                                       ast_add($$, ast_new("Vararg")); }
    ;

tableconstructor
    : '{' '}'                        { $$ = ast_new("Table"); }
    | '{' fieldlist '}'              { $$ = $2; }
    ;

fieldlist
    : fields                         { $$ = $1; }
    | fields fieldsep                { $$ = $1; }
    ;

fields
    : field                          { $$ = ast_new("Table"); ast_add($$, $1); }
    | fields fieldsep field          { $$ = $1; ast_add($$, $3); }
    ;

field
    : '[' exp ']' '=' exp            { $$ = ast_new("FieldKV");
                                       ast_add($$, $2); ast_add($$, $5); }
    | NAME '=' exp                   { $$ = ast_own("FieldName", $1);
                                       ast_add($$, $3); }
    | exp                            { $$ = $1; }
    ;

fieldsep
    : ','
    | ';'
    ;
```