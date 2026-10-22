//! Lua 5.3 发射器：把 CST 重新拼回可运行的 Lua。
//!
//! 三件事，别的原样透传：
//!   1. **擦除类型位**。CST 保留了全部标点 token，所以「打印」= 按序吐 token；
//!      类型语言只从七个带标签的产生式进入代码位，逐个跳过即可（见 table.rs 头注）：
//!      - 整段擦除 —— `@TypeAnnotation` / `@ReturnType` / `@Generics`
//!        以及整条纯类型语句 `@TypeDef` / `@Import` / `@Extern`
//!      - 留代码侧 —— `@Cast`(`e as T`→`e`) / `@TurboFish`(`e::<T>`→`e`) / `@VarargTyped`(`...T`→`...`)
//!   2. **降级 class**。class 是唯一需要真代码生成的构造：转成 class_base.lua 那套
//!      setmetatable OOP —— 但机制内联进每个 class 自己的定义里，不依赖外挂 prelude。
//!      构造 `A{…}` 在文法里和函数调用同形（`@Call(NAME, @Table)`），发射器不特判它，
//!      改让每个 class 表经 `__call` 可构造。
//!   3. **落 import 与前置声明**。import 来的类名换成
//!      `__typelua.get(模块, 类)`；同文件里声明前就被当值用到的类，在文件头
//!      建表占位。这两样都从 CST 现算，不需要 linter 的产出。
//!
//! 为什么现算就够了：保留名规则（见 type_linter 的 `check_reserved_names`）
//! 保证类名与 import 本地名在整个模块的值命名空间里唯一，不会有局部变量 /
//! 形参 / 全局赋值遮蔽它们。于是「这个名字是不是 import」只需查 import 语句，
//! 「这个类要不要前置」只需比较引用位置与声明位置 —— 都是文件内的语法事实

use super::super::utils::{DFS, string_literal};
use super::{EmitCtx, Emitter};
use crate::compiler::{RUNTIME_LOCAL, RUNTIME_MODULE};
use crate::lexer::type_def::*;
use crate::parser::ast::*;
use crate::parser::parser::*;
use crate::parser::table::*;
use std::collections::{HashMap, HashSet};

/// import 进来的一条：运行时去哪张表、以什么名字取。
/// `class` 是导出时的原名 —— 别名只改本文件的类型命名空间，注册表里的键是原名
struct ImportRef {
    module: String,
    class: String,
}

/// 发射期的本文件上下文。两项事实全部从 CST 现算：
///   - `imports`：顶层 `@Import` 语句里的本地名 -> (模块, 原名)
///   - `predeclares`：声明位置之前就被当值用到的本地类名，按首次引用排序
struct LowerCtx<'a> {
    module: &'a str,
    shared: bool,
    imports: HashMap<String, ImportRef>,
    predeclares: Vec<String>,
}

pub struct Lua53;

impl Lua53 {
    pub fn new() -> Self {
        Lua53
    }

    /// 不改自身：递归把 `node` 子树打成 Lua 文本追加进 `buffer`
    fn resolve(
        &self,
        ast: &Tree<NodeSyntax>,
        node: usize,
        buffer: &mut String,
        lower: &LowerCtx<'_>,
    ) {
        let data = ast.get_node(node).get_data();
        match data {
            NodeSyntax::Token(token) => self.emit_token(token, buffer),
            // 无标签内部节点：纯分组，原样透传子节点
            NodeSyntax::Prod(None) => self.emit_children(ast, node, buffer, lower),
            NodeSyntax::Prod(Some(prod)) => match prod {
                // —— 整段擦除：子树是纯类型 / 纯类型语句，运行时零痕迹 ——
                Prod::TypeAnnotation
                | Prod::ReturnType
                | Prod::Generics
                | Prod::TypeDef
                | Prod::Import
                | Prod::Extern => {}

                // —— 部分擦除：丢类型侧、留代码侧（都只留第 0 个孩子）——
                // `e as T` → `e`；`e::<T>` → `e`；`...T` → `...`
                Prod::Cast | Prod::TurboFish | Prod::VarargTyped => {
                    if let Some(&code) = ast.get_node(node).children.first() {
                        self.resolve(ast, code, buffer, lower);
                    }
                }

                // —— 降级：class 全套转成内联 setmetatable OOP ——
                Prod::ClassDecl | Prod::ClassDeclExtends => {
                    self.emit_class(ast, node, buffer, lower)
                }

                // import 来的类名在值位上要换成运行时取值。值位只有调用被调
                // 与静态读的基；写位 / 方法调用 / 下标由保留名规则挡在检查期
                Prod::VarRef => match self.value_site_import(ast, node, lower) {
                    // 走 push_token：它会在前面补空格。`return` 和 `__typelua`
                    // 挨在一起会被 Lua 当成一个名字
                    Some(imported) => {
                        let call = self.get_call(imported);
                        self.push_token(buffer, &call);
                    }
                    None => self.emit_children(ast, node, buffer, lower),
                },

                // 其余标签只是语义分组，子节点里已含全部标点 token：原样透传
                _ => self.emit_children(ast, node, buffer, lower),
            },
        }
    }

    /// `__typelua.get("pkg.account", "Account")` —— 值位上取一个 import 来的类。
    /// 运行时的 get 会先查已注册表，没注册才 require，且挡住循环 require
    fn get_call(&self, imported: &ImportRef) -> String {
        format!(
            "{RUNTIME_LOCAL}.get(\"{}\", \"{}\")",
            imported.module, imported.class
        )
    }

    fn emit_children(
        &self,
        ast: &Tree<NodeSyntax>,
        node: usize,
        buffer: &mut String,
        lower: &LowerCtx<'_>,
    ) {
        for &child in &ast.get_node(node).children {
            self.resolve(ast, child, buffer, lower);
        }
    }

    /// 单个 token 回写成源文本。不能用 `Token` 的 `Display`：它给 STRING 又套一层
    /// 引号、给单字符算子返回空串。token 之间一律补一个空格 —— Lua 空白不敏感，
    /// 留空格挡住相邻 token 意外粘连（`1 .. 2` 不塌成 `1..2`、两个 `-` 不拼成注释）
    fn emit_token(&self, token: &Token, buffer: &mut String) {
        let text: &str = match token {
            Token::STRING(s) => s, // 原文已含定界符
            Token::NUMERAL(n) => n,
            Token::NAME(n) => n,
            Token::RESERVED(r) => r.resolve(),
            Token::OPERATOR(OpType::SIMPLE(c)) => {
                let mut tmp = [0u8; 4];
                self.push_token(buffer, c.encode_utf8(&mut tmp));
                return;
            }
            Token::OPERATOR(o) => o.resolve(),
        };
        self.push_token(buffer, text);
        // `end` 收尾换行：给函数 / 分支 / 循环留点可读的行界，别的都堆在一行也合法
        if matches!(token, Token::RESERVED(Reserved::END)) {
            buffer.push('\n');
        }
    }

    fn push_token(&self, buffer: &mut String, text: &str) {
        if text.is_empty() {
            return;
        }
        // 只看末字节即可判空白：空白都是 ASCII，O(1)
        if buffer
            .as_bytes()
            .last()
            .is_some_and(|b| !b.is_ascii_whitespace())
        {
            buffer.push(' ');
        }
        buffer.push_str(text);
    }

    /// class 降级。两条产生式布局固定：
    ///   `@ClassDecl`        : [optpub, CLASS, NAME, generics, classbody]
    ///   `@ClassDeclExtends` : [optpub, CLASS, NAME, generics, ':', extendtype, classbody]
    ///
    /// 三处跟着现算的结果走：
    ///   - 被前置声明过的类只写 `名字 = {}`，`local` 已在文件头写过；
    ///     再写一次就成了另一个变量，闭包看到的仍是文件头那个
    ///   - 父类是 import 来的，`__index` / `__apply_defaults` 得走 `get`
    ///   - 共享模式下 pub 类要注册，别的模块的 `get` 才找得到
    fn emit_class(
        &self,
        ast: &Tree<NodeSyntax>,
        node: usize,
        buffer: &mut String,
        lower: &LowerCtx<'_>,
    ) {
        let is_extends = matches!(
            ast.get_node(node).get_data(),
            NodeSyntax::Prod(Some(Prod::ClassDeclExtends))
        );
        let name = self.name_of(ast, ast.get_node(node).children[2]);
        let (parent, body) = if is_extends {
            let pnode = ast.get_node(node).children[5];
            // 父类名字在运行时要的是一个**值**：import 来的换 get，本地的就是类名
            let expr = match self
                .child_name(ast, pnode, 0)
                .and_then(|n| lower.imports.get(n))
            {
                Some(imported) => self.get_call(imported),
                None => self.name_of(ast, pnode),
            };
            (Some(expr), ast.get_node(node).children[6])
        } else {
            (None, ast.get_node(node).children[4])
        };

        // 成员按源码顺序收齐（DFS 前序：@ListTail 左递归，老的在左、新的在右）
        let mut members = Vec::new();
        self.collect_members(ast, body, &mut members);

        // 类表本体 + 元信息
        if lower.predeclares.iter().any(|n| n == &name) {
            // 表已在文件头建好，这里只补内容：再 `local {name} = {{}}` 一次就是
            // 另一个变量，早先抓过去的引用（父类的 `__index`、闭包里的名字）全指着空表
            buffer.push('\n');
        } else {
            buffer.push_str(&format!("\nlocal {name} = {{}}\n"));
        }
        buffer.push_str(&format!("{name}.__index = {name}\n"));
        buffer.push_str(&format!("{name}.__ClassType__ = \"{name}\"\n"));

        // 方法（含静态方法）：直接挂到类表上
        for &m in &members {
            if matches!(
                ast.get_node(m).get_data(),
                NodeSyntax::Prod(Some(Prod::MethodDef))
            ) {
                self.emit_method(ast, &name, m, buffer, lower);
            }
        }

        // 字段默认值：没有构造器，默认值就是初始化。逐实例现求（`tags = {}` 每次新表），
        // 先补父类的再补自己的，缺省字段才不被覆盖
        buffer.push_str(&format!("function {name}.__apply_defaults(o)\n"));
        if let Some(ref p) = parent {
            buffer.push_str(&format!(
                "if {p} ~= nil and {p}.__apply_defaults then {p}.__apply_defaults(o) end\n"
            ));
        }
        for &m in &members {
            if matches!(
                ast.get_node(m).get_data(),
                NodeSyntax::Prod(Some(Prod::FieldDecl))
            ) && let Some((field, default)) = self.field_default(ast, m)
            {
                buffer.push_str(&format!("if o.{field} == nil then o.{field} = "));
                self.resolve(ast, default, buffer, lower);
                buffer.push_str(" end\n");
            }
        }
        buffer.push_str("end\n");

        // 构造 + 继承：类表的元表挂 __call（让 `A{…}` 能构造）与 __index（静态 / 方法沿父类查）
        buffer.push_str(&format!("setmetatable({name}, {{\n"));
        if let Some(ref p) = parent {
            buffer.push_str(&format!("__index = {p},\n"));
        }
        buffer.push_str("__call = function(cls, o)\n");
        buffer.push_str("o = o or {}\n");
        buffer.push_str("if cls.__apply_defaults then cls.__apply_defaults(o) end\n");
        buffer.push_str("return setmetatable(o, cls)\n");
        buffer.push_str("end,\n});\n");

        // 注册得放在最后：`get` 拿到的一定是挂完方法、设好元表的完整类
        if lower.shared && self.class_is_pub(ast, node) {
            buffer.push_str(&format!(
                "{RUNTIME_LOCAL}.def(\"{}\", \"{name}\", {name})\n",
                lower.module
            ));
        }
    }

    /// 这条 class 声明带了 `pub` 吗。`optpub` 恒在 children[0]，是 @Pub 节点才算
    fn class_is_pub(&self, ast: &Tree<NodeSyntax>, node: usize) -> bool {
        ast.get_node(node).children.first().is_some_and(|&c| {
            matches!(
                ast.get_node(c).get_data(),
                NodeSyntax::Prod(Some(Prod::Pub))
            )
        })
    }

    /// `@MethodDef` : [methodsig, block, END]。methodsig 首参名为 `self` ⇒ 实例方法，
    /// 用 `:` 语法糖自动补 self（发射时把显式 self 去掉）；否则是静态方法，用 `.`
    fn emit_method(
        &self,
        ast: &Tree<NodeSyntax>,
        class: &str,
        mdef: usize,
        buffer: &mut String,
        lower: &LowerCtx<'_>,
    ) {
        let methodsig = ast.get_node(mdef).children[0];
        let block = ast.get_node(mdef).children[1];
        let method = self.name_of(ast, methodsig); // methodsig 第 0 个孩子是方法名 NAME

        let mut params = Vec::new();
        self.collect_param_names(ast, methodsig, &mut params);
        let has_self = params.first().is_some_and(|p| p == "self");
        let sep = if has_self { ":" } else { "." };
        let shown = if has_self { &params[1..] } else { &params[..] };

        buffer.push_str(&format!(
            "function {class}{sep}{method}({}) ",
            shown.join(", ")
        ));
        self.resolve(ast, block, buffer, lower);
        buffer.push_str(" end\n");
    }

    /// 取一个节点代表的名字：节点本身是 NAME token，或它第 0 个孩子是 NAME token
    /// （@FieldDecl / @MethodSig / @Param / @TypeName / @GenericType 都是这个形状）
    fn name_of(&self, ast: &Tree<NodeSyntax>, node: usize) -> String {
        let n = ast.get_node(node);
        if let NodeSyntax::Token(Token::NAME(s)) = n.get_data() {
            return s.to_string();
        }
        if let Some(&first) = n.children.first()
            && let NodeSyntax::Token(Token::NAME(s)) = ast.get_node(first).get_data()
        {
            return s.to_string();
        }
        String::new()
    }

    /// 第 `i` 个孩子是 NAME 就取出来。和 `name_of` 的区别是明确要「第几个」，
    /// 取不到就是形状不对，返回 None 让调用点跳过而不是拿空串去查表
    fn child_name<'t>(
        &self,
        ast: &'t Tree<NodeSyntax<'_>>,
        node: usize,
        i: usize,
    ) -> Option<&'t str> {
        let &child = ast.get_node(node).children.get(i)?;
        match ast.get_node(child).get_data() {
            NodeSyntax::Token(Token::NAME(s)) => Some(s),
            _ => None,
        }
    }

    /// 节点的产生式标签。token 叶子给 None
    fn get_production(&self, ast: &Tree<NodeSyntax<'_>>, node: usize) -> Option<Prod> {
        match ast.get_node(node).get_data() {
            NodeSyntax::Prod(prod) => *prod,
            NodeSyntax::Token(_) => None,
        }
    }

    /// 摊平左递归列表脊，按源码顺序返回元素节点。形状和 linter 的 `spine`
    /// 一致：累加器在 children[0]、元素在末位，十一条列表都贴 @ListTail
    fn spine(&self, ast: &Tree<NodeSyntax<'_>>, node: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut cur = node;
        loop {
            let n = ast.get_node(cur);
            match n.get_data() {
                NodeSyntax::Prod(Some(Prod::ListTail)) => {
                    out.push(*n.children.last().unwrap());
                    cur = n.children[0];
                }
                // `fieldlist : fields fieldsep`：无标签两个符号，带着分隔符，继续下探
                NodeSyntax::Prod(None)
                    if n.children.len() == 2
                        && matches!(
                            ast.get_node(n.children[1]).get_data(),
                            NodeSyntax::Token(Token::OPERATOR(OpType::SIMPLE(',' | ';')))
                        ) =>
                {
                    cur = n.children[0]
                }
                _ => {
                    out.push(cur);
                    break;
                }
            }
        }
        out.reverse();
        out
    }

    /// chunk 的直接语句列表。根就是 chunk 的 @Block（`chunk : block` 被折叠了），
    /// children[0] 可能是 stat_list，也可能是 retstat —— 后者 spine 返回它自己
    fn top_level_stats(&self, ast: &Tree<NodeSyntax<'_>>) -> Vec<usize> {
        let Some(root) = ast.get_root() else {
            return Vec::new();
        };
        let Some(&stats) = ast.get_node(root).children.first() else {
            return Vec::new();
        };
        self.spine(ast, stats)
    }

    /// 把 classbody 子树里的 @FieldDecl / @MethodDef 摘出来（不钻进它们内部）
    fn collect_members(&self, ast: &Tree<NodeSyntax>, node: usize, out: &mut Vec<usize>) {
        match ast.get_node(node).get_data() {
            NodeSyntax::Prod(Some(Prod::FieldDecl)) | NodeSyntax::Prod(Some(Prod::MethodDef)) => {
                out.push(node);
            }
            _ => {
                for &child in &ast.get_node(node).children {
                    self.collect_members(ast, child, out);
                }
            }
        }
    }

    /// 从 methodsig 收形参名（含 `...`）。不钻进类型侧（@TypeAnnotation / @ReturnType /
    /// @Generics），否则会把类型里的名字或 `...` 误当形参
    fn collect_param_names(&self, ast: &Tree<NodeSyntax>, node: usize, out: &mut Vec<String>) {
        match ast.get_node(node).get_data() {
            NodeSyntax::Prod(Some(Prod::Param)) => out.push(self.name_of(ast, node)),
            NodeSyntax::Prod(Some(Prod::VarargTyped)) => out.push("...".to_string()),
            NodeSyntax::Token(Token::OPERATOR(OpType::ELLIPSIS)) => out.push("...".to_string()),
            NodeSyntax::Prod(Some(Prod::TypeAnnotation))
            | NodeSyntax::Prod(Some(Prod::ReturnType))
            | NodeSyntax::Prod(Some(Prod::Generics)) => {}
            _ => {
                for &child in &ast.get_node(node).children {
                    self.collect_param_names(ast, child, out);
                }
            }
        }
    }

    /// `@FieldDecl` 三形：`NAME ':' type` | `NAME ':' type '=' exp` | `NAME '=' exp`。
    /// 有 `=` 才有默认值，返回 (字段名, 默认值表达式节点)
    fn field_default(&self, ast: &Tree<NodeSyntax>, node: usize) -> Option<(String, usize)> {
        let children = &ast.get_node(node).children;
        for (i, &child) in children.iter().enumerate() {
            if let NodeSyntax::Token(Token::OPERATOR(OpType::SIMPLE('='))) =
                ast.get_node(child).get_data()
                && let Some(&expr) = children.get(i + 1)
            {
                return Some((self.name_of(ast, node), expr));
            }
        }
        None
    }
}

impl Lua53 {
    /// 顶层 import 语句里的本地名 -> 运行时坐标。别名取 `@ImportAlias` 的
    /// children[2]，普通 item 原名即本地名；模块名是 `@Import` 的 children[5]
    fn collect_imports(&self, ast: &Tree<NodeSyntax<'_>>, out: &mut HashMap<String, ImportRef>) {
        for stat in self.top_level_stats(ast) {
            if self.get_production(ast, stat) != Some(Prod::Import) {
                continue;
            }
            let Some(&str_node) = ast.get_node(stat).children.get(5) else {
                continue;
            };
            let NodeSyntax::Token(Token::STRING(raw)) = ast.get_node(str_node).get_data() else {
                continue;
            };
            let module = string_literal(raw);
            let Some(&list) = ast.get_node(stat).children.get(2) else {
                continue;
            };
            for item in self.spine(ast, list) {
                let Some(orig) = self.child_name(ast, item, 0) else {
                    continue;
                };
                let local = if self.get_production(ast, item) == Some(Prod::ImportAlias) {
                    match self.child_name(ast, item, 2) {
                        Some(local) => local,
                        None => continue,
                    }
                } else {
                    orig
                };
                out.insert(
                    local.to_string(),
                    ImportRef {
                        module: module.clone(),
                        class: orig.to_string(),
                    },
                );
            }
        }
    }

    /// 本文件要前置声明哪些类：顶层 class 里，**声明位置之前**就被当值
    /// 用到的名字。值位只有三处 —— 构造 / 静态读的基、`class B : A` 的父类。
    /// 引用位置用节点 span 比：`@VarRef` 的 span 就是名字本身，父类槽是
    /// children[5]。声明自身的体里用到自己不必前置（表在方法之前就建好了）
    fn collect_predeclares(&self, ast: &Tree<NodeSyntax<'_>>, out: &mut Vec<String>) {
        let mut decl_start: HashMap<&str, usize> = HashMap::new();
        for stat in self.top_level_stats(ast) {
            if matches!(
                self.get_production(ast, stat),
                Some(Prod::ClassDecl | Prod::ClassDeclExtends)
            ) && let Some(name) = self.child_name(ast, stat, 2)
            {
                decl_start.insert(name, ast.span_of(stat).start);
            }
        }
        if decl_start.is_empty() {
            return;
        }
        let Some(root) = ast.get_root() else {
            return;
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if let Some((site, name)) = self.class_value_site(ast, node)
                && decl_start
                    .get(name)
                    .is_some_and(|&start| ast.span_of(site).start < start)
                && seen.insert(name.to_string())
            {
                out.push(name.to_string());
            }
            for &child in ast.get_node(node).children.iter().rev() {
                stack.push(child);
            }
        }
    }

    /// 这个节点是不是「类名当值用」的位置，是就返回 (引用节点, 名字)。
    /// @VarRef 只在调用被调 / 静态读的基上算；extends 的父类槽单独认
    fn class_value_site<'t>(
        &self,
        ast: &'t Tree<NodeSyntax<'_>>,
        node: usize,
    ) -> Option<(usize, &'t str)> {
        match self.get_production(ast, node)? {
            Prod::VarRef => Some((node, self.value_site_name(ast, node)?)),
            Prod::ClassDeclExtends => {
                let &site = ast.get_node(node).children.get(5)?;
                Some((site, self.child_name(ast, site, 0)?))
            }
            _ => None,
        }
    }

    /// 这个 @VarRef 是「值位」吗：`A{…}` / `A(…)` 的被调，或 `A.m` 的基。
    /// 写位（`A = …`）、方法调用（`A:m()`）、下标（`A[1]`）都不算 —— 它们在
    /// 检查期就被拒绝了，发射端不必也不该改写
    fn value_site_name<'t>(&self, ast: &'t Tree<NodeSyntax<'_>>, node: usize) -> Option<&'t str> {
        if self.get_production(ast, node) != Some(Prod::VarRef) {
            return None;
        }
        let parent = ast.get_node(node).parent?;
        if ast.get_node(parent).children.first().copied() != Some(node) {
            return None;
        }
        if !matches!(
            self.get_production(ast, parent),
            Some(Prod::Call | Prod::Dot)
        ) {
            return None;
        }
        self.child_name(ast, node, 0)
    }

    /// 值位上的 @VarRef 如果指向 import 来的类，返回它的运行时坐标
    fn value_site_import<'l>(
        &self,
        ast: &Tree<NodeSyntax<'_>>,
        node: usize,
        lower: &'l LowerCtx<'_>,
    ) -> Option<&'l ImportRef> {
        let name = self.value_site_name(ast, node)?;
        lower.imports.get(name)
    }
}

impl DFS<()> for Lua53 {}

impl Emitter for Lua53 {
    /// 整棵树 -> 一份 Lua 文本。`write` 只是它再加一次落盘；拆开是为了
    /// 测试与宿主（C loader）不必碰文件系统
    fn render(&self, ast: &Tree<NodeSyntax>, ctx: &EmitCtx<'_>) -> String {
        let mut lower = LowerCtx {
            module: ctx.module,
            shared: ctx.shared,
            imports: HashMap::new(),
            predeclares: Vec::new(),
        };
        // 两项事实都只读树：import 了谁、哪个类在声明前就被用到
        self.collect_imports(ast, &mut lower.imports);
        self.collect_predeclares(ast, &mut lower.predeclares);

        let mut buffer = String::new();
        // 共享模式的文件头：拿到运行时句柄，声明本模块「已开始加载」。
        // enter 是循环 require 的挡板：本模块还在加载时别人来 get，会直接报
        // 「正在加载」而不是再 require 一遍
        if lower.shared {
            buffer.push_str(&format!(
                "local {RUNTIME_LOCAL} = require(\"{RUNTIME_MODULE}\")\n"
            ));
            buffer.push_str(&format!("{RUNTIME_LOCAL}.enter(\"{}\")\n", lower.module));
        }
        // 前置声明：单文件也要，和 shared 无关 —— 一个文件里两个类互相引用
        // 同样要求类表先占住名字。这里**直接把表建出来**（`local B = {{}}`），
        // 声明处只往里填：`setmetatable(A, {{__index = B}})` 抓到的是同一张表，
        // 前向继承才不会静默丢成 nil
        for name in &lower.predeclares {
            buffer.push_str(&format!("local {name} = {{}}\n"));
        }
        if let Some(root) = ast.get_root() {
            self.resolve(ast, root, &mut buffer, &lower);
        }
        buffer
    }
}

/// 发射端的回归测试。全部走「源码 -> 解析 -> 发射」：发射端只读 CST，
/// 所以测试也不该借 linter 的手
#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::emitter::emit_source;
    use crate::parser::parser::Parser;

    /// 把一组模块（模块名, 源码）各自解析并发射，返回各模块的 Lua 文本
    fn emit(mods: &[(&str, &str)], shared: bool) -> Vec<String> {
        let mut parsers: Vec<Parser> = mods.iter().map(|(_, src)| Parser::new(src)).collect();
        for parser in parsers.iter_mut() {
            parser.parse();
        }
        mods.iter()
            .zip(parsers.iter())
            .map(|((module, _), parser)| {
                Lua53::new().render(parser.tree(), &EmitCtx { module, shared })
            })
            .collect()
    }

    /// `emit_source` 是窄接口：只有源码与模块名，没有检查结果也能出 Lua
    #[test]
    fn emit_source_needs_no_checker() {
        let out = emit_source(
            "main",
            "import {B} in \"pkg.b\"\nlocal b = B {n = 1}",
            false,
        )
        .expect("语法该是好的");
        assert!(
            out.contains("__typelua.get(\"pkg.b\", \"B\") { n = 1 }"),
            "实得 {out}"
        );
    }

    /// 多模块：文件头拿到运行时、pub 类在自己的定义处登记、别处构造走 get
    #[test]
    fn shared_project_registers_and_fetches_classes() {
        let out = emit(
            &[
                ("pkg.acc", "pub class Account { id : number }"),
                (
                    "main",
                    "import {Account} in \"pkg.acc\"\nlocal a = Account {id = 1}\nprint(a.id)",
                ),
            ],
            true,
        );
        assert!(
            out[0].contains("__typelua.def(\"pkg.acc\", \"Account\", Account)"),
            "pub 类要登记，实得 {}",
            out[0]
        );
        assert!(
            out[1].starts_with("local __typelua = require(\"__typelua_runtime\")\n"),
            "文件头要先拿运行时，实得 {}",
            out[1]
        );
        assert!(
            out[1].contains("__typelua.enter(\"main\")"),
            "实得 {}",
            out[1]
        );
        assert!(
            out[1].contains("__typelua.get(\"pkg.acc\", \"Account\") { id = 1 }"),
            "构造要换成 get，实得 {}",
            out[1]
        );
    }

    /// 单文件不该看见运行时的任何痕迹
    #[test]
    fn single_module_output_stays_plain() {
        let out = emit(
            &[("main", "pub class A { n : number = 1 }\nlocal a = A{}")],
            false,
        );
        assert!(!out[0].contains("__typelua"), "实得 {}", out[0]);
    }

    /// 前向引用：类表在文件头就建好，声明处不再新建一张
    #[test]
    fn forward_reference_creates_the_table_up_front() {
        let out = emit(
            &[(
                "main",
                "class Z { make(self) -> A return A{} end }\nclass A { n : number = 1 }",
            )],
            false,
        );
        assert!(
            out[0].starts_with("local A = {}\n"),
            "要先占住名字，实得 {}",
            out[0]
        );
        assert_eq!(
            out[0].matches("local A = {}").count(),
            1,
            "只能建一张表，实得 {}",
            out[0]
        );
    }

    /// 前向继承：`__index` 抓到的是文件头那张表，不是 nil
    #[test]
    fn forward_extends_links_the_predeclared_table() {
        let out = emit(
            &[(
                "main",
                "class A : B { n : number = 1 }\nclass B { m : number = 2 }",
            )],
            false,
        );
        assert!(
            out[0].starts_with("local B = {}\n"),
            "父类要先建表，实得 {}",
            out[0]
        );
        assert!(out[0].contains("__index = B,"), "实得 {}", out[0]);
    }

    /// 类在自己体里的引用不必前置：表在方法定义之前就建好了
    #[test]
    fn self_reference_inside_the_body_needs_no_predeclare() {
        let out = emit(
            &[(
                "main",
                "class A { make(self) -> A return A{} end }\nlocal a = A{}",
            )],
            false,
        );
        assert!(
            !out[0].starts_with("local A = {}\n"),
            "不该前置，实得 {}",
            out[0]
        );
        assert_eq!(
            out[0].matches("local A = {}").count(),
            1,
            "声明处只该建一张表，实得 {}",
            out[0]
        );
    }

    /// 改写点前面要留空格：`return` 和 `__typelua` 粘在一起就是一个名字
    #[test]
    fn get_rewrite_does_not_glue_to_the_previous_token() {
        let out = emit(
            &[
                ("pkg.b", "pub class B { n : number = 1 }"),
                (
                    "main",
                    "import {B} in \"pkg.b\"\nclass A { make(self) -> B return B{} end }\nlocal a = A{}",
                ),
            ],
            true,
        );
        assert!(!out[1].contains("return__typelua"), "实得 {}", out[1]);
        assert!(
            out[1].contains("return __typelua.get(\"pkg.b\", \"B\")"),
            "实得 {}",
            out[1]
        );
    }
}
