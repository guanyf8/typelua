# typelua

## sample
```lua
-- ========================= Type declarations =========================
typedef Id      = number                                -- 标量也能命名（这是选 typedef 而非 interface 的原因）
typedef Handler = function(evt:string) -> ()            -- 函数类型
typedef Result  = A|B|nil                               -- 联合类型
typedef Config  = {host:string, port:number}            -- record
typedef Node    = {value:number, next:Node|nil}         -- 允许递归
typedef Pair<A, B> = {first:A, second:B}                -- 泛型
typedef Logger  = {                                     -- 描述外部已存在的表的形状
    log(...string),
    system_os() -> number,
    os:function() -> string,
}

-- ========================= Classes =========================
class A{
    a:string = "hello",                                 -- 有默认值
    b:boolean,                                          -- 无默认值 -> init 必须赋，否则报错
    c:number,
    greet:function(self:A, msg:string) -> string,       -- 成员函数【声明】：只给签名，没有函数体
    greet2(self, msg:string) -> number,                 -- 同上的简写；self 的类型自动是 A
    init(self, b:boolean, c:number)                     -- 构造器，编译器据此生成 A.new
        self.b = b
        self.c = c
    end,
    bye(self) -> string return "bye" end,               -- 成员函数【定义】：有 self -> 方法
    hello_again(self) -> (string, number) return "hello", 1 end,
    make(n:number) -> A return A.new(true, n) end,      -- 无 self -> 静态方法
    onclick:function(number) -> (),                     -- 无 self -> 普通函数字段（回调）
}

class B extends A{
    d:number = 0                                        -- 继承 A 的字段和方法
}

-- 成员函数也可以在类体外定义（Lua 的 ':' 形式，self 隐式）
function A:greet(msg:string) -> string
    return msg
end

local x = A.new(true, 1)
x:bye()                                                 -- 方法
x.onclick(42)                                           -- 回调字段：点号调用，不传 self
A.make(3)                                               -- 静态方法

-- ========================= Typed locals & globals =========================
local a:string = "hello"
local a:string, b:number, c:number = "world", 1, 2
local a = "hello"                                       -- 能推断的不必标注
-- local a                                              -- 不允许：无初始化必须声明类型
-- local n:number                                       -- 不允许：它实际是 nil，要写 number|nil
local p:{x:number, y:number} = {x = 1, y = 2}           -- 匿名 record
local pr:Pair<string, number> = {first = "a", second = 1}
local xs:array<string> = {}
local t:table<string, any> = {}                         -- 无初始化只对 record 型开放，这里给个初值
local h:Handler = function(evt:string) end
local E:Result
local _mytype:Logger                                    -- C 层注册的：record 类型可以只声明不赋值

G_config:Config = {host = "127.0.0.1", port = 80}       -- 全局变量也能标注

-- ========================= Functions =========================
-- 具名局部函数，参数带类型、含函数类型参数（可写裸类型），单返回值
local function f(a:string, b:number, c:function(string)) -> string
    return "hahaha"
end

-- 匿名函数赋值给变量，函数类型参数带返回值，多返回值
local g = function(a:string, c:function(string) -> boolean) -> (number, string)
    return 1, "x"
end

function noop() -> () end                               -- 无返回值(void)
function unpackall(t) -> (...any) return 1, 2, 3 end    -- 变长返回

function map<T, U>(t:array<T>, fn:function(T) -> U) -> array<U>
    local out:array<U> = {}
    for i:number, v:T in ipairs(t) do out[i] = fn(v) end -- 循环变量也能标注
    return out
end
local ys = map(xs, string.len)                          -- 推断 T=string, U=number
local zs = map::<string, number>(xs, string.len)        -- 推不出来时用 turbofish 指定

-- ========================= 多返回值 / 收窄 / 强转 =========================
local s1:string, n1:number = x:hello_again()            -- 展开
local s2 = (x:hello_again())                            -- 括号把它截断成 1 个值
local ok = pcall(noop)                                  -- 收多了照常丢弃

local s:string|nil = os.getenv("HOME")
if s ~= nil then print(#s) end                          -- nil 收窄，这里 s:string

local raw  = require("cjson")                           -- 推断为 any
local json = raw as {encode:function(any) -> string}    -- 用 as 落地
```


## typelua compared to lua
```
==================================================
约定：{A} 表示 0 个或多个 A；[A] 表示可选 A。
标记：  (=Lua)  与 Lua 原定义完全相同
        (~Lua)  在 Lua 基础上修改
        (+TLUA) TypeLua 新增

--------------------------------------------------
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
     for Name [‘:’ type] ‘=’ exp ‘,’ exp [‘,’ exp] do block end |
                                               -- (~Lua) 循环变量可带类型
     for decllist in explist do block end |     -- (~Lua) namelist -> decllist
     function funcname funcbody |
     local function Name funcbody |
     local decllist [‘=’ explist] |             -- (~Lua) namelist -> decllist
     class Name [generics] [extends Name] classbody |
                                               -- (+TLUA) 类声明
     typedef Name [generics] ‘=’ type           -- (+TLUA) 类型声明

retstat ::= return [explist] [‘;’]                                     (=Lua)

label ::= ‘::’ Name ‘::’                                               (=Lua)

funcname ::= Name {‘.’ Name} [‘:’ Name]                               (=Lua)

varlist ::= var {‘,’ var}                                              (=Lua)

var ::=  prefixexp [‘:’ type]                                          (~Lua)
     -- 可带类型标注，全局变量也因此能标注：`G:Config = load()`。
     -- 语义约束：标注只允许贴在裸 Name 上（`a.b.c:T` / `t[i]:T` 报错），
     --           且赋值目标不能是函数调用或括号表达式。
     -- 注：namelist 已删除，被 decllist 取代。

explist ::= exp {‘,’ exp}                                              (=Lua)

exp ::=  nil | false | true | Numeral | LiteralString | ‘...’ |        (~Lua)
     functiondef | prefixexp | tableconstructor |
     exp binop exp | unop exp |
     exp as type                                -- (+TLUA) 强转
     -- ‘as’ 的优先级最松：`a + b as T` 等于 `(a + b) as T`。
     -- 这样 `x as A|nil` 里的 ‘|’ 无歧义地归入类型，不必加括号 —— 而这是强转
     -- 最主要的用法。代价是 `(x as T) < y` 的括号不能省（否则 ‘<’ 会被当成
     -- 泛型的开启）。

prefixexp ::= var | functioncall | ‘(’ exp ‘)’ |                      (~Lua)
     prefixexp ‘::<’ typeargs ‘>’               -- (+TLUA) turbofish

functioncall ::=  prefixexp args | prefixexp ‘:’ Name args            (=Lua)

args ::=  ‘(’ [explist] ‘)’ | tableconstructor | LiteralString        (=Lua)

functiondef ::= function funcbody                                      (=Lua)

funcbody ::= [generics] ‘(’ [parlist] ‘)’ [rettype] block end          (~Lua)
     -- 加 [generics] 与 [rettype]

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
     --           则每个 Name 都必须带 `: type`；进一步地，该类型展开后必须是
     --           record（视为「外部提供的东西的形状」，信任它）。标量和 class
     --           会报错，因为它实际是 nil。判据是类型的形状，不是声明关键字。

-- ---- 泛型形参 ----
generics   ::= ‘<’ typeparams ‘>’
typeparams ::= Name {‘,’ Name}
     -- 泛型是真检查、不是只擦除。显式类型实参只能用 turbofish `::<>`，
     -- 不能用裸 `<>` —— 后者在表达式位置和比较运算真冲突，LALR(1) 分不开
     -- （TS 是靠手写 parser 回溯才勉强做到的）。

-- ---- 类型系统 ----
type      ::= uniontype
            | functype
uniontype ::= basictype {‘|’ basictype}       -- 联合类型（左结合、扁平化）
basictype ::= Name
            | nil                            -- nil 类型（用于可空/联合）
            | Name ‘<’ typeargs ‘>’          -- 泛型（支持任意嵌套）
            | ‘(’ type ‘)’                   -- 括号分组，如 (function()->A)|B
            | ‘{’ [classfieldlist] ‘}’       -- 匿名 record
     -- record 直接复用 classfieldlist，所以类体那套写法在类型位置原样可用；
     -- 类型位置只许「声明」形式，`= exp` 和 `block end` 由 checker 拒绝。
     -- `{...}` 归 record，所以数组/映射不给新语法，约定内建泛型名
     -- （array<T> / table<K,V>）即可。
typeargs  ::= type {‘,’ type}
functype  ::= function ‘(’ [argtypes] ‘)’ [rettype]

argtypes    ::= argtypelist [‘,’ vararg] | vararg
argtypelist ::= argtype {‘,’ argtype}
argtype     ::= Name ‘:’ type | type
     -- 类型位的参数表名字可省，所以不能复用 parlist。
     -- `function(string)` 是「参数类型为 string」，而不是「一个叫 string 的
     -- 无类型参数」—— 后者是 TS 的经典陷阱。带名形式纯属文档。

rettype   ::= ‘->’ retspec
retspec   ::= type                            -- 单返回值
            | ‘(’ ‘)’                        -- 0 个(void)
            | ‘(’ multilist ‘)’              -- 多返回值
multilist ::= typelist ‘,’ type               -- 两个或更多定长
            | typelist ‘,’ vararg            -- 定长 + 末位 vararg
            | vararg                          -- 只有 vararg
typelist  ::= type {‘,’ type}
     -- 单个定长返回走 basictype 的括号分组（`-> (T)` 等于 `-> T`），不在
     -- multilist 里；vararg 只许出现在末位，和 parlist 一致。
     -- 裸 ‘...’ 等价于 ‘...any’，没有「不检查」的特殊含义。
     -- ‘()’ 永远只是 retspec 语法、不是 type，永远不进 basictype ——
     -- 否则一旦它成为 unit 类型，`-> ()` 立刻歧义。

-- ---- 类 ----
classbody      ::= ‘{’ [classfieldlist] ‘}’
classfieldlist ::= classfield {‘,’ classfield} [‘,’]
classfield     ::= Name ‘:’ type [‘=’ exp]        -- 属性：声明[+默认值]
                 | Name ‘=’ exp                    -- 属性：定义(类型推断)
                 | methodsig [ block end ]         -- 方法：签名 / 内联定义
methodsig      ::= Name ‘(’ [parlist] ‘)’ [rettype]
     -- self 是显式的普通参数：第一个参数写 self 就是方法（其类型在类体内默认为
     -- 本类），不写就是静态方法。类型系统里没有「方法」这个概念，只有字段持有
     -- 函数值；`a:f(x)` 就是 `a.f(a, x)`，纯语法糖。
     -- 允许尾随逗号，但分隔符只能是 ‘,’ 而不是 fieldsep：‘;’ 既能当分隔符
     -- 又能当方法体里的空语句（`stat : ';'`），无函数体的 methodsig 后面
     -- 那个 ‘;’ 分不清是哪个。
     -- 字段是非空的：每个字段要么有默认值，要么被构造器在所有路径上赋值，
     -- 否则报错。构造器是保留的方法名 init，编译器据它生成 Name.new。

-- ---- 新增词法记号 ----
--   class     关键字
--   extends   关键字
--   typedef   关键字（不用 type：`type(x)` 是 Lua 标准库函数，不能被夺走）
--   as        关键字
--   ‘->’      返回类型箭头
--   ‘::<’     turbofish；三字符必须紧邻，`map:: <T>` 会被词法成 ‘::’ + ‘<’。
--             合法 Lua 里 ‘::’ 后必然跟 Name，所以 ‘::<’ 从不出现，最大匹配安全
-- 词法说明：在泛型 ‘<...>’ 内（generic_depth > 0，turbofish 同样开启这个上下文），
--           贪婪匹配出的 ‘>>’ 和 ‘>=’ 都会被拆回单个 ‘>’：
--             ‘>>’ → ‘>’ ‘>’   嵌套泛型闭合，如 map<string, list<number>>
--             ‘>=’ → ‘>’ ‘=’   闭合紧跟赋值，如 local t:table<string,any>=init()
--           三连 ‘>>=’（如 local x:T<U<V>>=t）由两条规则接力拆成 ‘>’ ‘>’ ‘=’。
--           拆分位置二选一：flex 版由 lexer 依 generic_depth 反馈拆分（见 ## lex）；
--           Rust 版 lexer 保持纯函数、统一产出 SHR/GE，由 parser 驱动层按 LALR
--           状态拆分 —— 判据是「‘>’ 有动作而 SHR/GE 是 error」，不能用「是否在
--           类型里」这种上下文标志，因为 turbofish 让泛型也出现在表达式位置。
```
## lex
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

"class"     { return CLASS; }      /* TLUA: class keyword                */
"extends"   { return EXTENDS; }    /* TLUA: inheritance keyword          */
"typedef"   { return TYPEDEF; }    /* TLUA: type declaration keyword     */
"as"        { return AS; }         /* TLUA: cast keyword                 */
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
"->"        { return ARROW; }      /* TLUA: return-type arrow            */
"::<"       { return TURBOFISH; }  /* TLUA: 显式类型实参；最大匹配，胜过 "::" */
"::"        { return DBCOLON; }
"=="        { return EQ; }
"~="        { return NE; }
"<="        { return LE; }
">="        { if (generic_depth > 0) { yyless(1); return '>'; }   /* TLUA: split generic close before '=' */
              return GE; }
"<<"        { return SHL; }
">>"        { if (generic_depth > 0) { yyless(1); return '>'; }   /* TLUA: split nested generic close */
              return SHR; }
"//"        { return IDIV; }

[-+*/%^#&~|<>=(){}\[\];:,.]   { return yytext[0]; }

.           { fprintf(stderr, "line %d: unexpected character '%s'\n", yylineno, yytext);
              return 0; }
```


## grammar
```bison
/* 运算符优先级：越靠后越紧。'as' 最松 —— `a + b as T` 是 `(a + b) as T`，
   这样 `x as A|nil` 里的 '|' 无歧义地归入类型，不必加括号 */
%left AS
%left OR
%left AND
%nonassoc '<' '>' LE GE EQ NE
%left '|'
%left '~'
%left '&'
%left SHL SHR
%right CONCAT
%left '+' '-'
%left '*' '/' IDIV '%'
%right UNARY
%right '^'

chunk
    : block                          { ast_root = $1; }
    ;

block
    : stat_list                      { $$ = $1; }
    | stat_list retstat              { $$ = $1; ast_add($$, $2); }
    | retstat                        { $$ = ast_new("Block"); ast_add($$, $1); }
    | /* empty */                    { $$ = ast_new("Block"); }
    ;

stat_list
    : stat                           { $$ = ast_new("Block"); ast_add($$, $1); }
    | stat_list stat                 { $$ = $1; ast_add($$, $2); }
    ;

stat
    : ';'                            { $$ = NULL; }
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
    | FOR NAME optype '=' exp ',' exp DO block END
                                     { $$ = ast_own("ForNum", $2);
                                       if ($3) ast_add($$, $3);
                                       ast_add($$, $5); ast_add($$, $7);
                                       ast_add($$, $9); }
    | FOR NAME optype '=' exp ',' exp ',' exp DO block END
                                     { $$ = ast_own("ForNum", $2);
                                       if ($3) ast_add($$, $3);
                                       ast_add($$, $5); ast_add($$, $7);
                                       Node *st = ast_new("Step"); ast_add(st, $9);
                                       ast_add($$, st); ast_add($$, $11); }
    | FOR decllist IN explist DO block END
                                     { $$ = ast_new("ForIn");
                                       ast_add($$, $2); ast_add($$, $4);
                                       ast_add($$, $6); }
    | FUNCTION funcname funcbody     { $$ = ast_own("Function", $2);
                                       ast_add($$, $3); }
    | LOCAL FUNCTION NAME funcbody   { $$ = ast_own("LocalFunction", $3);
                                       ast_add($$, $4); }
    | LOCAL decllist                 { if (!$2->aux) {
                                           yyerror("local without initializer must declare a type for every name");
                                           YYERROR;
                                       }
                                       $$ = ast_new("Local"); ast_add($$, $2); }
    | LOCAL decllist '=' explist     { $$ = ast_new("Local");
                                       ast_add($$, $2); ast_add($$, $4); }
    | CLASS NAME generics classbody  { $$ = ast_own("Class", $2);
                                       if ($3) ast_add($$, $3);
                                       ast_add($$, $4); }
    | CLASS NAME generics EXTENDS NAME classbody
                                     { $$ = ast_own("Class", $2);
                                       if ($3) ast_add($$, $3);
                                       ast_add($$, ast_own("Extends", $5));
                                       ast_add($$, $6); }
    | TYPEDEF NAME generics '=' type { $$ = ast_own("TypeDef", $2);
                                       if ($3) ast_add($$, $3);
                                       ast_add($$, $5); }
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
    : dotted_name                    { $$ = $1; }
    | dotted_name ':' NAME           { $$ = sconcat($1, ":", $3);
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
    : prefixexp optype               { if ($1->aux == PK_CALL || $1->aux == PK_PAREN) {
                                           yyerror("cannot assign to this expression (not a variable)");
                                           YYERROR;
                                       }
                                       if ($2 && strcmp($1->type, "Name") != 0) {
                                           yyerror("only a plain name can declare a type");
                                           YYERROR;
                                       }
                                       $$ = $1;
                                       if ($2) ast_add($$, $2); }
    ;

decllist
    : NAME optype                    { $$ = ast_new("NameList");
                                       Node *nm = ast_own("Name", $1);
                                       if ($2) ast_add(nm, $2);
                                       ast_add($$, nm);
                                       $$->aux = ($2 != NULL); }
    | decllist ',' NAME optype       { $$ = $1;
                                       Node *nm = ast_own("Name", $3);
                                       if ($4) ast_add(nm, $4);
                                       ast_add($$, nm);
                                       if (!$4) $$->aux = 0; }
    ;

optype
    : /* empty */                    { $$ = NULL; }
    | ':' type                       { $$ = $2; }
    ;

generics
    : /* empty */                    { $$ = NULL; }
    | '<' { generic_depth++; } typeparams '>'
                                     { generic_depth--; $$ = $3; }
    ;

typeparams
    : NAME                           { $$ = ast_new("TypeParams");
                                       ast_add($$, ast_own("TypeParam", $1)); }
    | typeparams ',' NAME            { $$ = $1;
                                       ast_add($$, ast_own("TypeParam", $3)); }
    ;

type
    : uniontype                      { $$ = $1; }
    | functype                       { $$ = $1; }
    ;

uniontype
    : basictype                      { $$ = $1; }
    | uniontype '|' basictype        { if (strcmp($1->type, "TypeUnion") == 0) {
                                           $$ = $1; ast_add($$, $3);
                                       } else {
                                           $$ = ast_new("TypeUnion");
                                           ast_add($$, $1); ast_add($$, $3);
                                       } }
    ;

basictype
    : NAME                           { $$ = ast_own("Type", $1); }
    | NIL                            { $$ = ast_dup("Type", "nil"); }
    | NAME '<' { generic_depth++; } typeargs '>'
                                     { generic_depth--;
                                       $$ = ast_own("Type", $1);
                                       ast_merge($$, $4); }
    | '(' type ')'                   { $$ = $2; }
    | '{' '}'                        { $$ = ast_new("Record"); }
    | '{' classfieldlist '}'         { $$ = ast_new("Record"); ast_merge($$, $2); }
    ;

typeargs
    : type                           { $$ = ast_new("TypeArgs"); ast_add($$, $1); }
    | typeargs ',' type              { $$ = $1; ast_add($$, $3); }
    ;

functype
    : FUNCTION '(' ')'               { $$ = ast_new("FuncType"); }
    | FUNCTION '(' ')' rettype       { $$ = ast_new("FuncType"); ast_add($$, $4); }
    | FUNCTION '(' argtypes ')'      { $$ = ast_new("FuncType"); ast_add($$, $3); }
    | FUNCTION '(' argtypes ')' rettype
                                     { $$ = ast_new("FuncType");
                                       ast_add($$, $3); ast_add($$, $5); }
    ;

argtypes
    : argtypelist                    { $$ = $1; }
    | argtypelist ',' vararg         { $$ = $1; ast_add($$, $3); }
    | vararg                         { $$ = ast_new("ArgTypes"); ast_add($$, $1); }
    ;

argtypelist
    : argtype                        { $$ = ast_new("ArgTypes"); ast_add($$, $1); }
    | argtypelist ',' argtype        { $$ = $1; ast_add($$, $3); }
    ;

argtype
    : NAME ':' type                  { $$ = ast_own("ArgName", $1); ast_add($$, $3); }
    | type                           { $$ = $1; }
    ;

rettype
    : ARROW retspec                  { $$ = $2; }
    ;

retspec
    : type                           { $$ = ast_new("Returns"); ast_add($$, $1); }
    | '(' ')'                        { $$ = ast_new("Returns"); }
    | '(' multilist ')'              { $$ = $2; }
    ;

multilist
    : typelist ',' type              { $$ = $1; ast_add($$, $3); }
    | typelist ',' vararg            { $$ = $1; ast_add($$, $3); }
    | vararg                         { $$ = ast_new("Returns"); ast_add($$, $1); }
    ;

typelist
    : type                           { $$ = ast_new("Returns"); ast_add($$, $1); }
    | typelist ',' type              { $$ = $1; ast_add($$, $3); }
    ;

classbody
    : '{' '}'                        { $$ = ast_new("ClassBody"); }
    | '{' classfieldlist '}'         { $$ = $2; }
    ;

classfieldlist
    : classfields                    { $$ = $1; }
    | classfields ','                { $$ = $1; }
    ;

classfields
    : classfield                     { $$ = ast_new("ClassBody"); ast_add($$, $1); }
    | classfields ',' classfield     { $$ = $1; ast_add($$, $3); }
    ;

classfield
    : NAME ':' type                  { $$ = ast_own("Field", $1); ast_add($$, $3); }
    | NAME ':' type '=' exp          { $$ = ast_own("Field", $1);
                                       ast_add($$, $3); ast_add($$, $5); }
    | NAME '=' exp                   { $$ = ast_own("Field", $1); ast_add($$, $3); }
    | methodsig                      { $$ = $1; }
    | methodsig block END            { $$ = $1; ast_add($$, $2); }
    ;

methodsig
    : NAME '(' ')'                   { $$ = ast_own("Method", $1); }
    | NAME '(' ')' rettype           { $$ = ast_own("Method", $1); ast_add($$, $4); }
    | NAME '(' parlist ')'           { $$ = ast_own("Method", $1); ast_add($$, $3); }
    | NAME '(' parlist ')' rettype   { $$ = ast_own("Method", $1);
                                       ast_add($$, $3); ast_add($$, $5); }
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
    | exp AS type                    { $$ = ast_new("Cast");
                                       ast_add($$, $1); ast_add($$, $3); }
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
    | prefixexp TURBOFISH { generic_depth++; } typeargs '>'
                                     { generic_depth--;
                                       $$ = ast_new("TurboFish");
                                       ast_add($$, $1); ast_add($$, $4);
                                       $$->aux = $1->aux; }
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
    : generics '(' ')' block END               { $$ = ast_new("FuncBody");
                                                 if ($1) ast_add($$, $1);
                                                 ast_add($$, $4); }
    | generics '(' ')' rettype block END       { $$ = ast_new("FuncBody");
                                                 if ($1) ast_add($$, $1);
                                                 ast_add($$, $4); ast_add($$, $5); }
    | generics '(' parlist ')' block END       { $$ = ast_new("FuncBody");
                                                 if ($1) ast_add($$, $1);
                                                 ast_add($$, $3); ast_add($$, $5); }
    | generics '(' parlist ')' rettype block END
                                               { $$ = ast_new("FuncBody");
                                                 if ($1) ast_add($$, $1);
                                                 ast_add($$, $3); ast_add($$, $5);
                                                 ast_add($$, $6); }
    ;

parlist
    : params                         { $$ = $1; }
    | params ',' vararg              { $$ = $1; ast_add($$, $3); }
    | vararg                         { $$ = ast_new("ParList"); ast_add($$, $1); }
    ;

params
    : param                          { $$ = ast_new("ParList"); ast_add($$, $1); }
    | params ',' param               { $$ = $1; ast_add($$, $3); }
    ;

param
    : NAME optype                    { $$ = ast_own("Name", $1);
                                       if ($2) ast_add($$, $2); }
    ;

vararg
    : ELLIPSIS                       { $$ = ast_new("Vararg"); }
    | ELLIPSIS type                  { $$ = ast_new("Vararg"); ast_add($$, $2); }
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