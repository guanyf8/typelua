//! Lua 5.3 发射器：把已过 linter 的 CST 重新拼回可运行的 Lua。
//!
//! 三件事，别的原样透传：
//!   1. **擦除类型位**。CST 保留了全部标点 token，所以「打印」= 按序吐 token；
//!      类型语言只从七个带标签的产生式进入代码位，逐个跳过即可（见 table.rs 头注）：
//!        整段擦除 —— `@TypeAnnotation` / `@ReturnType` / `@Generics`
//!                    以及整条纯类型语句 `@TypeDef` / `@Import` / `@Extern`
//!        留代码侧 —— `@Cast`(`e as T`→`e`) / `@TurboFish`(`e::<T>`→`e`) / `@VarargTyped`(`...T`→`...`)
//!   2. **降级 class**。class 是唯一需要真代码生成的构造：转成 class_base.lua 那套
//!      setmetatable OOP —— 但机制内联进每个 class 自己的定义里，不依赖外挂 prelude。
//!      构造 `A{…}` 在文法里和函数调用同形（`@Call(NAME, @Table)`），发射器不特判它，
//!      改让每个 class 表经 `__call` 可构造。
//!   3. **落计划**。linter 算好的两类事实在这里兑现：import 来的类名换成
//!      `__typelua.get(模块, 类)`，同文件的前向类引用靠文件头建表占位。
//!      两样都没有时（`shared == false` 且无前向引用）输出回到 master 的老样子

use super::super::plan::{ImportedClass, RUNTIME_LOCAL, RUNTIME_MODULE};
use super::super::utils::{DFS, Diagnostic};
use super::{EmitCtx, Emitter};
use crate::lexer::type_def::*;
use crate::parser::ast::*;
use crate::parser::parser::*;
use crate::parser::table::*;

pub struct Lua53 {
    diags: Vec<Diagnostic>,
}

impl Lua53 {
    pub fn new() -> Self {
        Lua53 { diags: Vec::new() }
    }

    /// 不改自身：递归把 `node` 子树打成 Lua 文本追加进 `buffer`
    fn resolve(
        &self,
        ast: &Tree<NodeSyntax>,
        node: usize,
        buffer: &mut String,
        diags: &mut Vec<Diagnostic>,
        ctx: &EmitCtx<'_>,
    ) {
        let data = ast.get_node(node).get_data();
        match data {
            NodeSyntax::Token(token) => self.emit_token(token, buffer),
            // 无标签内部节点：纯分组，原样透传子节点
            NodeSyntax::Prod(None) => self.emit_children(ast, node, buffer, diags, ctx),
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
                        self.resolve(ast, code, buffer, diags, ctx);
                    }
                }

                // —— 降级：class 全套转成内联 setmetatable OOP ——
                Prod::ClassDecl | Prod::ClassDeclExtends => {
                    self.emit_class(ast, node, buffer, diags, ctx)
                }

                // import 来的类名在值位上要换成运行时取值。linter 只给**值位**
                // 记改写点（构造 / 静态读；继承那处在 emit_class 里单独取），
                // 写位、funcname 与裸读根本不会进来
                Prod::VarRef => match ctx.plan.rewrite_at(node) {
                    // 走 push_token：它会在前面补空格。`return` 和 `__typelua`
                    // 挨在一起会被 Lua 当成一个名字
                    Some(imported) => self.push_token(buffer, &self.get_call(imported)),
                    None => self.emit_children(ast, node, buffer, diags, ctx),
                },

                // 其余标签只是语义分组，子节点里已含全部标点 token：原样透传
                _ => self.emit_children(ast, node, buffer, diags, ctx),
            },
        }
    }

    /// `__typelua.get("pkg.account", "Account")` —— 值位上取一个 import 来的类。
    /// 运行时的 get 会先查已注册表，没注册才 require，且挡住循环 require
    fn get_call(&self, imported: &ImportedClass) -> String {
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
        diags: &mut Vec<Diagnostic>,
        ctx: &EmitCtx<'_>,
    ) {
        for &child in &ast.get_node(node).children {
            self.resolve(ast, child, buffer, diags, ctx);
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
    /// 三处跟着计划走：
    ///   - 被前置声明过的类只写 `名字 = {}`，`local` 已在文件头写过；
    ///     再写一次就成了另一个变量，闭包看到的仍是文件头那个
    ///   - 父类是 import 来的，`__index` / `__apply_defaults` 得走 `get`
    ///   - 共享模式下 pub 类要注册，别的模块的 `get` 才找得到
    fn emit_class(
        &self,
        ast: &Tree<NodeSyntax>,
        node: usize,
        buffer: &mut String,
        diags: &mut Vec<Diagnostic>,
        ctx: &EmitCtx<'_>,
    ) {
        let is_extends = matches!(
            ast.get_node(node).get_data(),
            NodeSyntax::Prod(Some(Prod::ClassDeclExtends))
        );
        let name = self.name_of(ast, ast.get_node(node).children[2]);
        let (parent, body) = if is_extends {
            let pnode = ast.get_node(node).children[5];
            // 父类名字在运行时要的是一个**值**：import 来的换 get，本地的就是类名
            let expr = match ctx.plan.rewrite_at(pnode) {
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
        if ctx.plan.is_predeclared(&name) {
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
                self.emit_method(ast, &name, m, buffer, diags, ctx);
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
            ) {
                if let Some((field, default)) = self.field_default(ast, m) {
                    buffer.push_str(&format!("if o.{field} == nil then o.{field} = "));
                    self.resolve(ast, default, buffer, diags, ctx);
                    buffer.push_str(" end\n");
                }
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
        if ctx.shared && self.class_is_pub(ast, node) {
            buffer.push_str(&format!(
                "{RUNTIME_LOCAL}.def(\"{}\", \"{name}\", {name})\n",
                ctx.module
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
        diags: &mut Vec<Diagnostic>,
        ctx: &EmitCtx<'_>,
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
        self.resolve(ast, block, buffer, diags, ctx);
        buffer.push_str(" end\n");
    }

    /// 取一个节点代表的名字：节点本身是 NAME token，或它第 0 个孩子是 NAME token
    /// （@FieldDecl / @MethodSig / @Param / @TypeName / @GenericType 都是这个形状）
    fn name_of(&self, ast: &Tree<NodeSyntax>, node: usize) -> String {
        let n = ast.get_node(node);
        if let NodeSyntax::Token(Token::NAME(s)) = n.get_data() {
            return s.to_string();
        }
        if let Some(&first) = n.children.first() {
            if let NodeSyntax::Token(Token::NAME(s)) = ast.get_node(first).get_data() {
                return s.to_string();
            }
        }
        String::new()
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
            {
                if let Some(&expr) = children.get(i + 1) {
                    return Some((self.name_of(ast, node), expr));
                }
            }
        }
        None
    }
}

impl DFS<()> for Lua53 {}

impl Emitter for Lua53 {
    fn write(&self, ast: &Tree<NodeSyntax>, ctx: &EmitCtx<'_>, target_path: &str) -> bool {
        let buffer = self.render(ast, ctx);
        match std::fs::write(target_path, buffer) {
            Ok(()) => true,
            Err(err) => {
                eprintln!("{}: 写出失败 {target_path}: {err}", ctx.module);
                false
            }
        }
    }
}

impl Lua53 {
    /// 整棵树 -> 一份 Lua 文本。`write` 只是它再加一次落盘；拆开是为了
    /// 测试不必碰文件系统
    fn render(&self, ast: &Tree<NodeSyntax>, ctx: &EmitCtx<'_>) -> String {
        let mut buffer = String::new();
        let mut diags = Vec::new();
        // 共享模式的文件头：拿到运行时句柄，声明本模块「已开始加载」。
        // enter 是循环 require 的挡板：本模块还在加载时别人来 get，会直接报
        // 「正在加载」而不是再 require 一遍
        if ctx.shared {
            buffer.push_str(&format!(
                "local {RUNTIME_LOCAL} = require(\"{RUNTIME_MODULE}\")\n"
            ));
            buffer.push_str(&format!("{RUNTIME_LOCAL}.enter(\"{}\")\n", ctx.module));
        }
        // 前置声明：单文件也要，和 shared 无关 —— 一个文件里两个类互相引用
        // 同样要求类表先占住名字。这里**直接把表建出来**（`local B = {{}}`），
        // 声明处只往里填：`setmetatable(A, {{__index = B}})` 抓到的是同一张表，
        // 前向继承才不会静默丢成 nil
        for name in &ctx.plan.predeclares {
            buffer.push_str(&format!("local {name} = {{}}\n"));
        }
        if let Some(root) = ast.get_root() {
            self.resolve(ast, root, &mut buffer, &mut diags, ctx);
        }
        buffer
    }
}
