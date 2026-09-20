//! Lua 5.3 发射器：把已过 linter 的 CST 重新拼回可运行的 Lua。
//!
//! 两件事，别的原样透传：
//!   1. **擦除类型位**。CST 保留了全部标点 token，所以「打印」= 按序吐 token；
//!      类型语言只从七个带标签的产生式进入代码位，逐个跳过即可（见 table.rs 头注）：
//!        整段擦除 —— `@TypeAnnotation` / `@ReturnType` / `@Generics`
//!                    以及整条纯类型语句 `@TypeDef` / `@Import` / `@Extern`
//!        留代码侧 —— `@Cast`(`e as T`→`e`) / `@TurboFish`(`e::<T>`→`e`) / `@VarargTyped`(`...T`→`...`)
//!   2. **降级 class**。class 是唯一需要真代码生成的构造：转成 class_base.lua 那套
//!      setmetatable OOP —— 但机制内联进每个 class 自己的定义里，不依赖外挂 prelude。
//!      构造 `A{…}` 在文法里和函数调用同形（`@Call(NAME, @Table)`），发射器不特判它，
//!      改让每个 class 表经 `__call` 可构造。

use super::super::utils::{DFS, Diagnostic};
use super::Emitter;
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
    ) {
        let data = ast.get_node(node).get_data();
        match data {
            NodeSyntax::Token(token) => self.emit_token(token, buffer),
            // 无标签内部节点：纯分组，原样透传子节点
            NodeSyntax::Prod(None) => self.emit_children(ast, node, buffer, diags),
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
                        self.resolve(ast, code, buffer, diags);
                    }
                }

                // —— 降级：class 全套转成内联 setmetatable OOP ——
                Prod::ClassDecl | Prod::ClassDeclExtends => {
                    self.emit_class(ast, node, buffer, diags)
                }

                // 其余标签只是语义分组，子节点里已含全部标点 token：原样透传
                _ => self.emit_children(ast, node, buffer, diags),
            },
        }
    }

    fn emit_children(
        &self,
        ast: &Tree<NodeSyntax>,
        node: usize,
        buffer: &mut String,
        diags: &mut Vec<Diagnostic>,
    ) {
        for &child in &ast.get_node(node).children {
            self.resolve(ast, child, buffer, diags);
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
    fn emit_class(
        &self,
        ast: &Tree<NodeSyntax>,
        node: usize,
        buffer: &mut String,
        diags: &mut Vec<Diagnostic>,
    ) {
        let is_extends = matches!(
            ast.get_node(node).get_data(),
            NodeSyntax::Prod(Some(Prod::ClassDeclExtends))
        );
        let name = self.name_of(ast, ast.get_node(node).children[2]);
        let (parent, body) = if is_extends {
            (
                Some(self.name_of(ast, ast.get_node(node).children[5])),
                ast.get_node(node).children[6],
            )
        } else {
            (None, ast.get_node(node).children[4])
        };

        // 成员按源码顺序收齐（DFS 前序：@ListTail 左递归，老的在左、新的在右）
        let mut members = Vec::new();
        self.collect_members(ast, body, &mut members);

        // 类表本体 + 元信息
        buffer.push_str(&format!("\nlocal {name} = {{}}\n"));
        buffer.push_str(&format!("{name}.__index = {name}\n"));
        buffer.push_str(&format!("{name}.__ClassType__ = \"{name}\"\n"));

        // 方法（含静态方法）：直接挂到类表上
        for &m in &members {
            if matches!(
                ast.get_node(m).get_data(),
                NodeSyntax::Prod(Some(Prod::MethodDef))
            ) {
                self.emit_method(ast, &name, m, buffer, diags);
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
                    self.resolve(ast, default, buffer, diags);
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
        self.resolve(ast, block, buffer, diags);
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
    fn write(&self, ast: &Tree<NodeSyntax>, module_name: &str, target_path: &str) {
        let mut buffer = String::new();
        let mut diags = Vec::new();
        if let Some(root) = ast.get_root() {
            self.resolve(ast, root, &mut buffer, &mut diags);
        }
        if let Err(err) = std::fs::write(target_path, buffer) {
            eprintln!("{module_name}: 写出失败 {target_path}: {err}");
        }
    }
}
