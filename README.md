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
typedef Bounded<T:number> = {v:T}                       -- 形参上界
typedef Meta = {VERSION:number} & table<string, any>    -- 交类型：形状合并
typedef Logger  = {                                     -- 描述外部已存在的表的形状
    log:function(...string),                            -- 类型位只能写函数变量字段：
    system_os:function() -> number,                     -- 「只给签名的方法」这种东西不存在
    os:function() -> string,
}

-- ========================= Classes =========================
-- 类成员分两种，**互不为糖**：
--   `m(self) … end`  是**方法**：必须带函数体（只给签名不合法），不参与 `A{…}`
--                    构造，也只有它能被类体外的 `function A:m` / `function A.m` 重定义
--   `m : function(…)` 是**函数变量字段**：和 string / number 一样的普通字段，有默认值
--                    就随实例带一份、没有就每处构造都得给；不能用 `function` 去定义它
class A{
    a:string = "hello",                                 -- 有默认值
    b:boolean,                                          -- 无默认值 -> 每处初始化都必须给
    c:number,
    greet(self, msg:string) -> string                   -- 有 self -> 方法；self 的类型自动是 A
        return msg
    end,
    bye(self) -> string return "bye" end,
    hello_again(self) -> (string, number) return "hello", 1 end,
    make(n:number) -> A return A{b = true, c = n} end,  -- 无 self -> 静态方法
    onclick:function(number) -> () = function(n:number) end,
                                                        -- 函数变量字段（回调）：这里给了默认值，
                                                        -- 不给默认值就得每处构造都传
}

class B : A{
    d:number = 0                                        -- 继承 A 的字段和方法
}

-- 方法可以在类体外**重定义**（Lua 的 ':' 形式，self 隐式补在形参表头）。
-- 名字必须是类体里声明过的方法，签名也得对得上
function A:greet(msg:string) -> string
    return msg
end

local x = A {
    b = true,
    c= 1
}
-- local bad = A { b = true }                           -- 不允许：c 无默认值，没给就拒绝
-- local bad = A { b = true, c = 1, zz = 9 }            -- 不允许：zz 不是 A 的字段
-- local bad = A { b = true, c = 1, bye = f }           -- 不允许：bye 是方法，构造里给不了
-- function A:absent() end                              -- 不允许：类体里没声明过这个方法
-- function A:onclick(n:number) end                     -- 不允许：onclick 是函数变量字段
-- A.greet = f                                          -- 不允许：A 是类名而不是变量
-- x.greet = f                                          -- 不允许：方法不能赋值覆盖
local y = B { b = false, c = 2 }                        -- 继承来的非空字段照样要给；d 有默认值
x:bye()                                                 -- 方法
x.onclick(42)                                           -- 回调字段：点号调用，不传 self
A.make(3)                                               -- 静态方法

-- ========================= Typed locals & globals =========================
local a:string = "hello"
local w:string, m:number, k:number = "world", 1, 2     -- 一句声明多个，各带各的类型
local guess = "hello"                                   -- 能推断的不必标注
-- local a                                              -- 不允许：无初始化必须声明类型
-- local a  local a                                     -- 不允许：同层不许重复声明（Lua 能遮蔽，TypeLua 拒）
local n:number                                          -- OK：无初值，读之前必须赋值（定值分析盯着）
local p:{x:number, y:number} = {x = 1, y = 2}           -- 匿名 record
local pr:Pair<string, number> = {first = "a", second = 1}
local h:Handler = function(evt:string) end
local E:Result                                          -- OK：Result 含 nil，声明出来就已经是合法值

-- 空表 {} 推不出形状，所以必须标注，且标注得是「能空着的类型」
local xs:array<string> = {}                             -- 集合：元素后加
local t:table<string, any> = {}                         -- 映射：键后加
-- local m = {}                                         -- 不允许：{} 推不出形状
-- local r:Config = {}                                  -- 不允许：host/port 是非空字段，没给
local cfg:Config                                        -- 表也能只声明不赋值
cfg = {host = "h", port = 80}                           -- 这一句才是它的构造点
-- local r2:Config  print(r2.host)                      -- 不允许：r2 尚未赋值就被读
-- local r3:Config  r3.host = "h"                       -- 不允许：同上，逐项注入也是一次读
local _mytype = _G.logger as Logger                     -- 已有表达式来源时用 as 落地

-- 宿主（C 层）注入的全局用 extern 声明：只说形状，不核实存在性，零代码生成
extern my_global:Logger                                 -- 之后 my_global.log(...) 照常查类型
extern c_alloc:function(number) -> any
my_global = _G.other as Logger                          -- 可以赋值，但类型不能背叛声明

G_config:Config = {host = "127.0.0.1", port = 80}       -- 全局变量也能标注
-- G_cache = {}                                         -- 不允许：全局裸赋值同一条规则
G_cache:table<string, Config> = {}

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

-- 全局函数声明参与抬升，所以 Lua 的互递归写法原样保留（pong 的声明点在后面）
function ping(n:number) -> number
    if n > 0 then return pong(n - 1) end
    return 0
end
function pong(n:number) -> number return ping(n) end

local function fact(n:number) -> number                 -- 自递归：名字在进入函数体前已登记
    if n <= 1 then return 1 end
    return n * fact(n - 1)
end
-- local function fact2(n) return n * fact2(n - 1) end  -- 不允许：递归函数必须标注返回类型

-- 局部互递归：local function 不抬升，照 Lua 的规矩写前向声明
local step1:function(number) -> number
local step2:function(number) -> number
step1 = function(n:number) -> number return step2(n - 1) end
step2 = function(n:number) -> number
    if n > 0 then return step1(n) end
    return 0
end
-- local function bad1() -> number return bad2() end    -- 不允许：bad2 在 Lua 里会被编译成
-- local function bad2() -> number return bad1() end       全局读，运行时炸 nil

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

表的形状一次定死

四条规则（推翻了早期「先声明再逐项注入」那一版）：
  - `{}` 推不出形状，所以**空表字面量必须有标注**：`local a = {}` 和全局裸 `a = {}`
    都拒。非空字面量不受此限，它自己能定形。
  - 标注得是「能空着的类型」：`array<T>` / `table<K,V>`（元素、键后加）或空 record `{}`
    （终态，永远没有字段）。带非空字段的 record / class 拿 `{}` 初始化是错的 ——
    字段没给，和 `A{}` 少字段是同一条诊断。
  - **不存在「先声明再逐项注入字段」**：record 一旦定形就长不出新成员，
    `local M:{} = {}` 之后 `M.VERSION = 1` 报「`{}` 上没有字段 VERSION」。模块表要么一次
    写全成一个完整字面量，要么用 `table<K,V>`，要么两者交起来：
    `typedef Mod = {VERSION:number} & table<string, function() -> ()>`。
  - 无初值的 `local x:T` 里 T **可以**是表，但形状仍然只从构造点来：`local r:Config` 之后
    必须**整体赋一次** `r = {…}`，`r.host = "h"` 不算 —— 那是一次「读 r」，而 r 此刻还没赋值。
    于是上一条不必单独立规则，它是「先声明后使用」的推论（见下面「无初值声明」）。
    描述外部已存在的表另有两条路：手里已经有表达式来源时用 `as`（`_G.logger as Logger`）；
    宿主注入的全局用 `extern`（见下节）。

不管辖的：`a.b = {}` / `a[1] = {}`。它们的类型由所属 record / 集合定，而且文法本来就
只许把标注贴在裸 Name 上。

先声明后使用

所有变量都必须先声明后使用，全局也不例外。四条规则：
  - **局部变量**的声明点是 `local decllist [‘=’ explist]`，作用域从该语句**之后**开始 ——
    和 Lua 一致（`local x = x` 右边那个 x 是外层的）。
  - **全局变量**的声明点是它第一次出现的地方，而且**只能在文件顶层**：顶层的 `G:T = exp`
    （带标注）或 `G = exp`（能推断）。嵌在任何 block 里的赋值只是赋值，找不到声明就报错 ——
    不给「在 if 里第一次赋值也算声明」留口子，否则声明点取决于控制流，判据会一路滑向
    可达性分析，而读代码的人也无从判断 G 到底是谁的。
  - 后续赋值可以在任意地方，但**不能背叛声明**：走和 `extern` 同一条兼容性检查。
  - `extern Name ‘:’ type` 是第三种声明形态，同样只能顶层。它声明存在性而不核实存在性。
标准库的全局（print / require / ipairs / string / os / tostring / pcall / _G / …）由一份
内建 prelude 提前声明，形态上等价于一串 `extern`，所以不必在每个文件顶上重复写。这份
prelude 是「先声明后使用」的必要配件，没有它每个文件都会报一屏。

无初值声明

`local x:T` 可以不给初值，**T 是什么都行**，唯一的硬要求是标注得在：
  - `local n:number` / `local f:function() -> ()` / `local r:Config` —— 都可以。
  - `local a` —— 不行，没标注就没类型可言。
代价是每个变量多带一位「已赋过值」，**读一个尚未赋值的变量是错**（TS2454
「used before being assigned」）。这一位把 soundness 拉回来了：类型说 `number` 而
运行时是 nil 的那段窗口里，变量根本不允许被读。
老规则「无初值就要求 T 容得下 nil」不再是门槛，而是降级成**这一位的初值**：
`local E:Result`（`A|B|nil`）声明出来就算已赋值 —— nil 真是它的合法值，读它安全；
`local n:number` 从未赋值起步。

何处查：只在**立即求值位置**查，**函数体内一律不查**。后一条不是偷懒：
`a = function() b() end` 里的 `b` 此刻当然还没赋值，查了就把局部互递归当场判死。
这和下面「抬升只在延迟求值位置生效」是同一条线，TS 也是这么划的
（`let x:number; const f = () => x + 1; x = 1;` 在 TS 里不报）。

分支怎么合：赋值只会把这一位从「未赋」推向「已赋」，所以不建控制流图，
一遍遍历配上快照 / 回滚 / 取交集就够：
  - `if c then n=1 else n=2 end` 过；`if c then n=1 end` 报 —— 落空路径没赋值。
  - `while` / `for` 的体可能执行 0 次，体内的赋值不往外传；`do` / `repeat` 至少
    一次，往外传。
  - 走不出来的分支不参与交集，所以 guard 写法照常：
    `if c then n=1 else return end` 、`if c then n=1 else error("bad") end` 都过。
    后一例靠的是 prelude 把 `error` / `os.exit` 声明成返回 `never`（底类型，
    一个值也没有）：调用求不出值来，于是这一支走不到下一句。
    判据在类型上、不在写法上，所以 `os.exit()`（字段读）和 `local e = error`
    之后的 `e("x")` 一样算。反过来，`assert` 不能这么声明 —— 条件为真时它正常返回。
    `never` 是个普通的内建类型名（和 `array` / `table` 同列），各个类型位都写得出，
    否则 prelude 自己就无法声明这类函数。它没有值，所以 `local x:never` 声明得出来
    却永远赋不上，读它就报「可能尚未赋值」—— 无需为它另立规则。
  - 函数里一出现 `goto`，该函数的这项检查降级为放过 —— 向后跳让线性合并失效，
    宁可漏报不误报。

为什么不要求写 `|nil` 再收窄：**代价不对称**。record 的收窄能在一处包住一整段代码，而互递归
的调用点在**别的函数体里**，收窄跨不过函数边界，每个调用点都得重写一遍 `if a ~= nil then`。
所以这里选择放过，而不是让惯用写法付仪式钱。换来的是 Lua 的局部互递归能原样写
（见 sample 里的 step1 / step2）。
全局没有对应形态 —— 文法上全局的声明点必然带 `= explist`，所以全局互递归走下面的抬升。

函数声明抬升

Lua 里 `function a() b() end function b() a() end` 合法（`a` 体内的 `b` 是 `_ENV.b` 的
运行时查找），这是最常见的互递归写法。为了不砍掉它，**顶层的全局函数声明参与抬升**：
`function Name funcbody` 的名字在整个文件可见，不必等到它的声明语句。四条边界：
  - 只抬升 `function Name funcbody`（funcname 是裸 Name）。`function M.a()` / `function A:f()`
    不引入新名字，它们是给已有变量的字段赋值，字段由 `M` / `A` 的类型给出，本来就不需要抬升。
  - **`local function` 不抬升**。Lua 的局部名字是词法解析的：`local function a() b() end`
    里的 `b`，若 `local b` 出现在它之后，编译出来的是 `_ENV.b` 而不是那个局部 —— 抬升它
    等于放行一份运行时必炸的代码。`local function f` 只享有「名字在进入函数体之前就已登记」
    这一条（手册规定它展开成 `local f; f = function…`，而不是 `local f = function…`），
    所以**自递归**可用、互递归得靠上面的前向声明。
  - **函数表达式不抬升**：`local g = function() … end` 的 `g` 是普通局部变量，声明点就是那行。
  - 抬升只让名字在**延迟求值位置**（函数体内）可见。顶层立即求值的位置引用一个声明点还在
    后面的函数，原生 Lua 也是运行时炸 nil，checker 照 TS2448 的形状报「b 在此处尚未赋值」。

递归函数的返回类型

没有 `->` 标注时返回类型靠体内的 return 表达式推断，而自引用会让这个推断依赖自己。所以
**参与递归（自递归或互递归）的函数必须显式标注返回类型**，和 TS7023 同一个动机，标注即解环。
参数类型不受影响：签名在进入函数体之前就已登记，递归调用的实参照常检查。

宿主声明 extern

FFI 是 Lua 的本职，所以宿主（C 层）注入的全局得能在 TypeLua 里声明出来，而不是只能靠
`_G.x as T` 绕。形态就一条语句：`extern Name ‘:’ type`。五条规则：
  - **只能在顶层**。和普通全局变量一样由 checker 拦 —— 文法层做不到：把带标注的赋值拆出
    一份顶层专用版会和 `var ::= prefixexp [optype]` 撞出 reduce/reduce（`optype` 可空，
    两条路推出同一个串），实测过。
  - **不核实存在性**。编译器不去证明这个全局真的被注入了，那是宿主的责任，和 `as` 同一个
    信任模型。但类型名本身要能解析：`extern x:NoSuchType` 是笔误，报错。
  - **不 pub**。它声明的是变量本身而不是类型，动态对象不参与导出；`pub extern` 是语法错
    （产生式里没给 optpub 槽）。这一条不靠检查，靠文法保证。
  - **零代码生成**。它只往类型环境里写一条，抹除后一个字不剩 —— 这是它和
    `local x = _G.x as T`（生成一行真赋值）的本质区别。
  - **可以赋值**，覆盖那个全局；但类型不能背叛声明，赋值走和带标注全局同一条兼容性检查。

跨文件/跨模块

三条规则：
  - **静态面**（`class` / `typedef`）由 `pub` 决定是否跨文件可见，`import` 引入；纯编译期，擦除后一个字都不剩
  - **动态面**（实例变量）走 Lua 原本的 `require` / `return`，typelua 不插手
  - 两者互不寄生：只用类型不必 `require`，只用值不必 `import`

1. 文件 `mod_a.lua`
```lua
pub class A {                                           -- pub：类型名可被别的文件 import
    a:string,
    b:number,
    f(self, s:string, n:number) -> boolean return false end,
}

pub typedef Shape = {                                   
    g:function(s:string, n:number) -> number,           -- 类型位只有函数变量字段
}

typedef Internal = number                             
class Helper { v:number }                      

local M = A { a = "hello", b = 1 }                      -- 类字面量：方法有内联定义就不必再给

-- 支持类体外重定义，只要签名对得上
function A:f(s:string, n:number) -> boolean
    return true
end

return M                                                -- 动态面：只返回实例变量
```

2. 文件 `mod_b.lua`
```lua
import {A, Shape} in "mod_a"                            -- 只引入类型名
import {Shape as S2, A as A2} in "mod_a"                -- 撞名时用 as 改名

local m:A = require "mod_a"                            
local ok:boolean = m:f("x", 1)

local sh:S2 = {                                         
    g = function(s:string, n:number) -> number return 1 end,
}

-- import {Internal} in "mod_a"                         -- 不允许：Internal 没有 pub
-- local bad = A { a = "x", b = 1 }                     -- 不允许：import 只带类型不带值。
--                                                         要构造就用 mod_a 导出的工厂函数
-- class C : A2 { d:number = 0 }                        -- 同上：继承要父类的运行时值
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
     function funcname funcbody |              -- (~Lua) 顶层裸名参与抬升，见「函数声明抬升」
     local function Name funcbody |            -- (~Lua) 不抬升，只保证名字先于函数体登记
     local decllist [‘=’ explist] |             -- (~Lua) namelist -> decllist
     [pub] class Name [generics] [‘:’ extendtype] classbody |
                                               -- (+TLUA) 类声明；pub = 跨文件可见
     [pub] typedef Name [generics] ‘=’ type |   -- (+TLUA) 类型声明；pub 同上
     import ‘{’ importlist ‘}’ in LiteralString | -- (+TLUA) 类型导入
     extern Name ‘:’ type                       -- (+TLUA) 宿主声明；只顶层，checker 拦

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

-- ---- 跨文件 ----
importlist ::= importitem {‘,’ importitem} [‘,’]
importitem ::= Name | Name as Name            

decllist ::= Name [‘:’ type] {‘,’ Name [‘:’ type]}
     -- 无初值时标注必给，类型不限；读之前必须赋过值（见「无初值声明」）。

-- ---- 泛型形参 ----
generics   ::= ‘<’ typeparams ‘>’
typeparams ::= typeparam {‘,’ typeparam}
typeparam  ::= Name | Name ‘:’ type          -- 上界；与 `class B : A` 同一个符号
     -- 上界里的 ‘:’ 是 ‘:’ 的第五次复用，它在 ‘<’ ‘>’ 内部，和父类槽那个
     -- 分属两个状态，`class A<T: number> : B {}` 不歧义（实测零新冲突）。
     -- 泛型是真检查、不是只擦除。显式类型实参只能用 turbofish `::<>`，
     -- 不能用裸 `<>` —— 后者在表达式位置和比较运算真冲突，LALR(1) 分不开
     -- （TS 是靠手写 parser 回溯才勉强做到的）。

extendtype ::= Name | Name ‘<’ typeargs ‘>’   -- 父类槽：可开泛型实参

-- ---- 类型系统 ----
type      ::= uniontype
            | functype
uniontype ::= intertype {‘|’ intertype}       -- 联合类型（左结合、扁平化）
intertype ::= basictype {‘&’ basictype}       -- 交类型：形状合并，比 ‘|’ 紧
     -- `A & B | C` 就是 `(A&B) | C`。‘&’ 不新增词法记号 —— 它本来就是 Lua 的按位与，
     -- 和 ‘|’ 一样是类型位与值位共用一个记号（两边分得开：`as` 绑最松）。
     -- 主用途是「带具名字段的集合」：`{VERSION:number} & table<string, any>`，
     -- 以及给集合接元表：`table<K,V> & {__add:function(T, T) -> T}`。
basictype ::= Name
            | nil                            -- nil 类型（用于可空/联合）
            | Name ‘<’ typeargs ‘>’          -- 泛型（支持任意嵌套）
            | ‘(’ type ‘)’                   -- 括号分组，如 (function()->A)|B
            | ‘{’ [classfieldlist] ‘}’       -- 匿名 record
     -- 四个标量名（any / boolean / number / string）、两个内建泛型名
     -- （array / table）和 `never` 都只是预先注册好的 Name，不是关键字 ——
     -- 用户自己声明的同名类型遮蔽它们。`never` 是底类型（一个值也没有），
     -- 主用于返回位：`extern error:function(any) -> never` 告诉 checker
     -- 调它就回不来，详见「无初值声明」。checker 内部还有个 `unknown`
     -- （「还没算出来」），它**不在**这张表里：能写就等于给了一个关掉检查的后门。
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
                 | methodsig block end             -- 方法：必带函数体
methodsig      ::= Name ‘(’ [parlist] ‘)’ [rettype]
     -- 前两条是**属性**，第三条是**方法**，两者不互为糖：
     --   属性就是普通字段，类型写 function(…) 就是个函数变量字段，和 string /
     --   number 一样待：没默认值就每处构造都得给，一人一份，不能用 function 定义；
     --   方法是类上的行为，不参与构造，但可以在类体外被 `function A:m` / `function A.m`
     --   重定义（名字必须先在类体里声明过，签名也得对得上）。
     -- 所以「只给签名的方法」这条候选式不存在：methodsig 必须跟着 block end。
     -- 这也顺手封住了类型位：basictype 的 ‘{’ classfieldlist ‘}’ 和类体共用
     -- 同一个 classfieldlist，而匿名 record 里的函数体归谁、何时查都没答案 ——
     -- 想要个函数成员就写属性 `m : function(…)`。
     -- self 是显式的普通参数：第一个参数写 self 就是方法（其类型在类体内默认为
     -- 本类），不写就是静态方法。类型系统里没有「方法」这个概念，只有字段持有
     -- 函数值；`a:f(x)` 就是 `a.f(a, x)`，纯语法糖。「是不是方法」只活在声明层。
     -- 允许尾随逗号，但分隔符只能是 ‘,’ 而不是 fieldsep。当初的硬理由是 ‘;’ 既能当
     -- 分隔符又能当方法体里的空语句（`stat : ‘;’`），无函数体的 methodsig 后面那个
     -- ‘;’ 分不清是哪个；那条候选式现在删了，留下的理由只是一致性 ——
     -- 类体是声明表，而 parlist / typelist / record 这些声明位一律只用 ‘,’。
     -- 字段是非空的：每个字段要么有默认值，要么在**每一处**初始化里给出，否则报错。
     -- 没有构造器，也没有 Name.new：初始化照结构体字面量来 ——
     --     local x = A{b = true, c = 1}
     -- 这条不需要任何新语法。`A{…}` 就是 `prefixexp args` 里 `args ::= tableconstructor`
     -- 那条（Lua 自带的 `f{…}` 糖），归约出来的节点和函数调用完全一样，由 checker
     -- 看 prefixexp 是不是类名来区分「构造实例」和「拿一张表去调函数」。
     -- 子类字面量直接把父类的非空字段一起写出来，没有构造链、不需要 super。
     -- 同一条原则往下推到普通表：`{}` 推不出形状，所以空表字面量必须带标注，
     -- 而且 record 一旦定形就长不出新成员（详见上面「表的形状一次定死」）。
     --
     -- 【父类为什么用 ‘:’ 而不是 extends 关键字】‘:’ 已经在这门语言里表示「有此
     -- 类型」（`x:T`），而 class 就是它的 record 形状，所以「B 是一个 A」是同一个
     -- 关系，写 `class B : A` 自洽。当时实测是纯替换：状态数、产生式数不变，
     -- 只少一个终结符，移进-归约冲突仍是 ‘(’ 上那两个。
     -- 换来的是 extends 被释放回普通标识符 —— `local extends = 1`、`t:extends()`
     -- 都合法，这对一门要和现存 Lua 代码共处的语言是实收益。
     -- 连带好处：泛型形参上界直接写 `<T : Cmp>`（已落地，实测零新冲突），不必为它
     -- 单独留一个关键字；否则 extends 的唯一残留用途就只是那个。
     -- 代价是 ‘:’ 反复复用（类型标注 / 方法调用 / funcname / 继承 / 形参上界，共五处），且
     -- `class B : A{` 和类字面量 `A{…}` 形似 —— 建议写成 `class B : A {`。
     --
     -- 父类槽只收一个 extendtype（`A` 或 `A<…>`）：**单实现继承**，所以继承图是一棵树，
     -- 菱形不可能出现。开泛型实参是为了让三种形态都能写：
     --     class NamedBox<T> : Box<T>      -- 实参透传
     --     class IntBox      : Box<number> -- 实参固定
     --     class Flipped<A,B>: Pair<B, A>  -- 实参重排
     -- 不能直接拿 basictype 当父类槽：它的 ‘{’ classfieldlist ‘}’ 会和紧跟在后面的
     -- classbody 撞成移进冲突。多重「符合某形状」也不走这里 —— record 是结构类型，
     -- 形状对得上就能赋值过去，本来就不需要声明。若将来要 `: A, IFoo` 的列表，
     -- 实测再 +3 状态 +2 产生式。

-- ---- 新增词法记号 ----
--   六个关键字（class / typedef / as / pub / import / extern）+ 两个符号（‘->’ / ‘::<’）。
--   交类型的 ‘&’ 、联合的 ‘|’ 、上界与继承的 ‘:’ 全是复用，零新增记号。
--   class     关键字
--   typedef   关键字（不用 type：`type(x)` 是 Lua 标准库函数，不能被夺走）
--   as        关键字（强转 + import 改名，两处共用）
--   pub       关键字（导出修饰；实测某 SDK 里仅 1 处 `params.pub` 字段位冲突）
--   import    关键字（类型导入；该 SDK 里 71 处冲突全在 `t.import` 字段位，
--             故建议一并放开「保留字可出现在 ‘.’ 后与表键位」，那条能一次救回全部）
--   extern    关键字（宿主声明；只能在顶层，由 checker 拦。见上面「宿主声明 extern」一节）
--   ‘->’      返回类型箭头
--   ‘::<’     turbofish；三字符必须紧邻，`map:: <T>` 会被词法成 ‘::’ + ‘<’。
--             合法 Lua 里 ‘::’ 后必然跟 Name，所以 ‘::<’ 从不出现，最大匹配安全
-- 词法说明：在泛型 ‘<...>’ 内（generic_depth > 0，turbofish 同样开启这个上下文），
--           贪婪匹配出的 ‘>>’ 和 ‘>=’ 都会被拆回单个 ‘>’：
--             ‘>>’ → ‘>’ ‘>’   嵌套泛型闭合，如 map<string, list<number>>
--             ‘>=’ → ‘>’ ‘=’   闭合紧跟赋值，如 local t:table<string,any>=init()
--           拆分是逐个记号做的，所以连着多层也成：‘>>=’（`local x:T<U<V>>=t`）拆成
--           ‘>’ ‘>’ ‘=’；四层嵌套 `Pair<array<number>, table<string, array<Pair<number,
--           string>>>>` 的尾巴同理。两份实现都实测过。
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
"typedef"   { return TYPEDEF; }    /* TLUA: type declaration keyword     */
"as"        { return AS; }         /* TLUA: cast keyword                 */
"pub"       { return PUB; }        /* TLUA: 导出修饰符                   */
"import"    { return IMPORT; }     /* TLUA: 类型导入                     */
"extern"    { return EXTERN; }     /* TLUA: 宿主声明                     */
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
                                   /* ':' 兼五职：类型标注 `x:T`、方法调用 `a:f()`、
                                      funcname 的 `A:m`、TLUA 的继承 `class B : A`、
                                      TLUA 的形参上界 `<T : Cmp>`。五处的上下文互不相交，
                                      LALR(1) 分得开（实测零新冲突）。
                                      '|' 和 '&' 同样两职：值位是按位或/与，类型位是联合/交。
                                      词法不区分，交由文法分 —— 这也是 `as` 必须绑最松的原因 */

.           { fprintf(stderr, "line %d: unexpected character '%s'\n", yylineno, yytext);
              return 0; }
```


## grammar
```bison
/* 这份是另一种编码：exp 扁平、靠 %left/%right 消解，而 src/parser/table.rs 里那份
   是阶梯式的（cast_exp / or_exp / … / pow_exp）。同一门语言，两种写法。
   实测（bison 2.3）：本扁平版 5 个 shift/reduce、0 个 reduce/reduce；
   阶梯版 2 个 shift/reduce、0 个 reduce/reduce。多出来的三个全是 `as` 带的：
     两个 Lua 自带的（两版共有）：`stat : prefixexp ·` / `exp : prefixexp ·` 遇 ‘(’
     `basictype : NAME ·`      遇 ‘<’ —— `x as T<U>` 的 ‘<’ 也可以是比较
     `type : uniontype ·`     遇 ‘|’ —— `x as A|B` 的 ‘|’ 也可以是按位或
     `uniontype : intertype ·` 遇 ‘&’ —— `x as A&B` 的 ‘&’ 也可以是按位与
   三者都落在没声明优先级的产生式上，优先级消解不介入，bison 一律默认移进，
   结果与阶梯版一致（阶梯里 cast_exp 在最外层，强转结果当不了二元运算的左操作数，
   那三个根本不存在）。两份都实测能解析 README 全部 sample 与 examples/*.tlua。
   `extern`（宿主声明）已落地到 src/parser/table.rs；加上它两版冲突数均不变，
   已实测（阶梯 2、扁平 5）。它只能出现在顶层这条靠 checker，不在本文法里。 */

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
    | optpub CLASS NAME generics classbody
                                     { $$ = ast_own("Class", $3);
                                       if ($3) ast_add($$, $3);
                                       ast_add($$, $4); }
    | optpub CLASS NAME generics ':' extendtype classbody
                                     { $$ = ast_own("Class", $3);
                                       if ($4) ast_add($$, $4);
                                       ast_add($$, ast_own("Extends", $6));
                                       ast_add($$, $6); }
      /* 父类槽收 extendtype（`A` 或 `A<…>`）—— 单实现继承，所以继承图是一棵树，
         菱形不可能出现。不能直接拿 basictype：它的 '{' classfieldlist '}' 会和紧跟在
         后面的 classbody 撞成移进冲突。将来若要 `: A, IFoo` 的列表，实测再 +3 状态
         +2 产生式。 */
    | optpub TYPEDEF NAME generics '=' type
                                     { $$ = ast_own("TypeDef", $3);
                                       if ($4) ast_add($$, $4);
                                       ast_add($$, $6); }
    | IMPORT '{' importlist '}' IN STRING
                                     { $$ = ast_new("Import");
                                       ast_add($$, $3); ast_add($$, ast_own("Module", $6)); }
    | EXTERN NAME ':' type           /* 见上面「宿主声明 extern」一节 */
                                     { $$ = ast_own("Extern", $2); ast_add($$, $4); }
    ;

/* pub 做成可空前缀而不是复制一份产生式：class/typedef 各只保留一个标签，
   语义层看第 0 个孩子有没有 Pub 即可，decl / emitter 都不必分叉 */
optpub
    : /* empty */                    { $$ = NULL; }
    | PUB                            { $$ = ast_new("Pub"); }
    ;

/* 分隔符用 IN（已是 Lua 关键字），不用 from —— 后者在现存 Lua 里当裸变量用得太多 */
importlist
    : importitems                    { $$ = $1; }
    | importitems ','                { $$ = $1; }
    ;

importitems
    : importitem                     { $$ = ast_new("ImportList"); ast_add($$, $1); }
    | importitems ',' importitem     { $$ = $1; ast_add($$, $3); }
    ;

importitem
    : NAME                           { $$ = ast_own("ImportName", $1); }
    | NAME AS NAME                   { $$ = ast_own("ImportAlias", $1);
                                       ast_add($$, ast_own("Name", $3)); }
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
    : typeparam                      { $$ = ast_new("TypeParams"); ast_add($$, $1); }
    | typeparams ',' typeparam       { $$ = $1; ast_add($$, $3); }
    ;

typeparam
    : NAME                           { $$ = ast_own("TypeParam", $1); }
    | NAME ':' type                  { $$ = ast_own("TypeParamBound", $1);
                                       ast_add($$, $3); }
    ;

/* 父类槽专用：只许 `A` / `A<…>`，不走 basictype（见上面 ClassDeclExtends 那条） */
extendtype
    : NAME                           { $$ = ast_own("Type", $1); }
    | NAME '<' { generic_depth++; } typeargs '>'
                                     { generic_depth--;
                                       $$ = ast_own("Type", $1);
                                       ast_merge($$, $4); }
    ;

type
    : uniontype                      { $$ = $1; }
    | functype                       { $$ = $1; }
    ;

uniontype
    : intertype                      { $$ = $1; }
    | uniontype '|' intertype        { if (strcmp($1->type, "TypeUnion") == 0) {
                                           $$ = $1; ast_add($$, $3);
                                       } else {
                                           $$ = ast_new("TypeUnion");
                                           ast_add($$, $1); ast_add($$, $3);
                                       } }
    ;

/* 交类型比 '|' 紧：`A & B | C` = `(A&B) | C` */
intertype
    : basictype                      { $$ = $1; }
    | intertype '&' basictype        { if (strcmp($1->type, "TypeIntersect") == 0) {
                                           $$ = $1; ast_add($$, $3);
                                       } else {
                                           $$ = ast_new("TypeIntersect");
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
    /* 前三条是**属性**（普通字段，含函数变量字段）：第一条无默认值，
       checker 要求每一处 `Name{…}` 都给出它；后两条带默认值。
       第四条是**方法**，methodsig 必须带函数体 —— 「只有签名」那条候选式
       删掉了，因为方法和函数变量字段不互为糖，只给签名的函数成员得写成
       属性 `m : function(…)`。init 不是保留的方法名 —— 没有构造器了，
       初始化是字面量（见 prefixexp args）。 */
    : NAME ':' type                  { $$ = ast_own("Field", $1); ast_add($$, $3); }
    | NAME ':' type '=' exp          { $$ = ast_own("Field", $1);
                                       ast_add($$, $3); ast_add($$, $5); }
    | NAME '=' exp                   { $$ = ast_own("Field", $1); ast_add($$, $3); }
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
      /* (+TLUA) 类初始化 `A{…}` 也归到这条：args 的第三条候选式就是
         tableconstructor（Lua 自带的 `f{…}` 糖），所以「构造实例」和「拿表调
         函数」在语法上完全同形，这里分不开、也不该分 —— 交给 checker 看 $1
         是不是类名。零新产生式、零新冲突，代价只是 AST 上没有专门的节点。
         `Box::<string>{…}` 同样自然：TURBOFISH 那条之后再接这条即可。 */
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
      /* 空表那条语法上永远合法，拦它的是 checker：`{}` 推不出形状，所以
         `local a = {}` / 全局裸 `a = {}` 要求必须有标注，且标注得能空着
         （array<T> / table<K,V> / 空 record `{}`）。详见「表的形状一次定死」。 */

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