//! plugin_linter：给编辑器插件用的独立分析器 —— 只做两件事，语义高亮
//! （span -> 种类）与跳转定义（引用 span -> 定义位置），和 type_linter 完全隔离：
//! 不碰 Session / TypeId / 类型系统，也不接进 LintDriver 那套 Linter 流水线，
//! 只读 CST。遍历复用 utils 里的通用 `walk`，自带一套轻量的块级作用域。
//!
//! 隔离的代价是它得把 type_linter 里那几个「按 CST 形状认节点」的纯查询
//! （get_production / spine / is_chunk_top …）各自再抄一份精简版 —— 这些只读
//! ast、不认识类型，抄过来不会把类型系统倒灌进这个模块，正是隔离的意义。
//!
//! 跨模块跳转（用户选的档位）：先扫全工程建一份「模块 -> 顶层 class/typedef 名
//! -> 定义 span」的导出索引，再逐文件分析；`import {A as B} in "mod"` 的 A
//! 顺着这份索引连回它在别的文件里的定义。因此对外入口是工程级的 analyze_project。

use super::super::utils::{DFS, Diagnostic, walk};
use crate::compiler::utils::string_literal;
use crate::lexer::type_def::{Span, Token};
use crate::parser::ast::Tree;
use crate::parser::parser::*;
use crate::parser::table::Prod;
use std::collections::HashMap;

// ================== 对外数据模型 ==================

/// 语义 token 的种类。对齐 LSP 的 semantic token 常见枚举，编辑器那侧照名字
/// 建 legend 即可。标点 / 结构符不产出（编辑器用 TextMate 兜），语义高亮只管
/// 关键字、字面量、注释，以及按角色分好类的标识符
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TokenKind {
    Keyword,
    String,
    Number,
    Comment,
    Type,          // class / typedef / 类型位的名字引用
    TypeParameter, // 泛型形参 <T>
    Function,      // 函数声明名、被当函数调用的名字
    Method,        // a:m() 的 m、方法声明名
    Property,      // a.b 的 b、字段声明、表构造里的具名键
    Parameter,     // 形参
    Variable,      // 局部/全局变量引用
    Label,         // goto / ::label::
}

impl TokenKind {
    /// JSON 里用的名字（也就是编辑器 legend 里的 token type 名）
    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Keyword => "keyword",
            TokenKind::String => "string",
            TokenKind::Number => "number",
            TokenKind::Comment => "comment",
            TokenKind::Type => "type",
            TokenKind::TypeParameter => "typeParameter",
            TokenKind::Function => "function",
            TokenKind::Method => "method",
            TokenKind::Property => "property",
            TokenKind::Parameter => "parameter",
            TokenKind::Variable => "variable",
            TokenKind::Label => "label",
        }
    }
}

/// 一个高亮片段：源码字节区间 + 种类。区间是字节偏移，行列/UTF-16 换算交给
/// 编辑器那侧（它手里有文档全文，`positionAt` 一步到位），Rust 这边不掺和
pub struct SemToken {
    pub span: Span,
    pub kind: TokenKind,
}

/// 一条跳转：引用出现在 `span`，定义落在 `target_module` 的 `target` 处。
/// 同文件跳转时 target_module 就是本模块名
pub struct DefLink {
    pub span: Span,
    pub target_module: String,
    pub target: Span,
}

/// 单个文件分析完的产物
pub struct FileAnalysis {
    pub module: String,
    pub tokens: Vec<SemToken>,
    pub links: Vec<DefLink>,
}

// ================== 工程级入口 ==================

/// 一处定义的位置（跨模块用得着模块名）
#[derive(Clone)]
struct Target {
    module: String,
    span: Span,
}

/// 作用域里一个变量符号：定义 span + 高亮该用的种类（函数/形参/普通变量）
#[derive(Clone, Copy)]
struct VarSym {
    span: Span,
    kind: TokenKind,
}

/// 工程级分析器。先 `index` 扫遍每个模块攒出导出索引，再 `analyze` 逐文件出结果
pub struct PluginAnalyzer {
    /// 模块名 -> (顶层 class/typedef 名 -> 定义 span)。class 名和 typedef 名
    /// 共一张表 —— typelua 里类型不分命名空间
    exports: HashMap<String, HashMap<String, Span>>,
}

impl PluginAnalyzer {
    pub fn new() -> Self {
        PluginAnalyzer {
            exports: HashMap::new(),
        }
    }

    /// 扫一棵树，把它顶层的 class / typedef 名登记进导出索引。只认 chunk 顶层：
    /// 嵌在块里的类型不是 `import ... in "mod"` 找得到的那些
    pub fn index(&mut self, ast: &Tree<NodeSyntax<'_>>, module: &str) {
        let Some(root) = ast.get_root() else { return };
        let mut found: Vec<(String, Span)> = Vec::new();
        each_node(ast, root, &mut |n| {
            if matches!(
                get_production(ast, n),
                Some(Prod::ClassDecl | Prod::ClassDeclExtends | Prod::TypeDef)
            ) && is_chunk_top(ast, n)
                && let Some(name_node) = ast.get_node(n).children.get(2).copied()
                && let Some(name) = get_name(ast, name_node)
            {
                found.push((name.to_string(), ast.span_of(name_node)));
            }
        });
        let table = self.exports.entry(module.to_string()).or_default();
        for (name, span) in found {
            table.insert(name, span);
        }
    }

    /// 分析一个文件。`comments` 是 parser 交回的注释 span 列表（词法期收集的），
    /// 直接铺成 Comment 高亮
    pub fn analyze(
        &self,
        ast: &Tree<NodeSyntax<'_>>,
        module: &str,
        comments: &[Span],
    ) -> FileAnalysis {
        let mut a = Analysis {
            module: module.to_string(),
            exports: &self.exports,
            tokens: Vec::new(),
            links: Vec::new(),
            var_scopes: vec![HashMap::new()],
            type_scopes: vec![HashMap::new()],
            current_import: None,
        };

        // 注释先铺上，顺序无所谓 —— 消费端自己按位置排
        for &c in comments {
            a.tokens.push(SemToken {
                span: c,
                kind: TokenKind::Comment,
            });
        }

        if let Some(root) = ast.get_root() {
            a.hoist_top_level(ast, module, root);
            let mut diags: Vec<Diagnostic> = Vec::new();
            walk(&mut a, &mut (), ast, root, &mut diags);
        }

        FileAnalysis {
            module: module.to_string(),
            tokens: a.tokens,
            links: a.links,
        }
    }
}

impl Default for PluginAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// 工程级一把梭：先对所有模块 index（攒导出索引），再逐个 analyze。
/// 每项是 (模块名, 语法树, 该文件的注释 span)
pub fn analyze_project(modules: &[(&str, &Tree<NodeSyntax<'_>>, Vec<Span>)]) -> Vec<FileAnalysis> {
    let mut analyzer = PluginAnalyzer::new();
    for (module, ast, _) in modules {
        analyzer.index(ast, module);
    }
    modules
        .iter()
        .map(|(module, ast, comments)| analyzer.analyze(ast, module, comments))
        .collect()
}

// ================== 单文件遍历器 ==================

/// per-file 的遍历状态。跨文件不复用（导出索引才是跨文件的那份），所以每次
/// analyze 现建一个。作用域两条独立的栈：变量名与类型名各查各的
struct Analysis<'e> {
    module: String,
    exports: &'e HashMap<String, HashMap<String, Span>>,
    tokens: Vec<SemToken>,
    links: Vec<DefLink>,
    /// 变量作用域栈：名字 -> (定义 span, 高亮种类)
    var_scopes: Vec<HashMap<String, VarSym>>,
    /// 类型作用域栈：名字 -> 定义位置（本文件或跨模块）。只放「跳得过去」的，
    /// 内建类型 / 没解析上的引用干脆不进表，查不到就只高亮不连线
    type_scopes: Vec<HashMap<String, Target>>,
    /// 正在处理的 import 语句的目标模块名（`in "mod"` 那个）。进 @Import 置上、
    /// 出 @Import 清掉，中间的 importitem 靠它连回导出索引
    current_import: Option<String>,
}

impl<'e> Analysis<'e> {
    // ---- 产出 ----

    fn emit(&mut self, span: Span, kind: TokenKind) {
        self.tokens.push(SemToken { span, kind });
    }

    /// 给某个节点的 span 打高亮，返回该 span 供连线复用
    fn emit_node(&mut self, ast: &Tree<NodeSyntax<'_>>, node: usize, kind: TokenKind) -> Span {
        let span = ast.span_of(node);
        self.emit(span, kind);
        span
    }

    fn link(&mut self, span: Span, target: Target) {
        self.links.push(DefLink {
            span,
            target_module: target.module,
            target: target.span,
        });
    }

    // ---- 作用域 ----

    fn resolve_var(&self, name: &str) -> Option<VarSym> {
        self.var_scopes
            .iter()
            .rev()
            .find_map(|s| s.get(name).copied())
    }

    fn type_target(&self, name: &str) -> Option<Target> {
        self.type_scopes
            .iter()
            .rev()
            .find_map(|s| s.get(name).cloned())
    }

    fn declare_var(&mut self, name: &str, sym: VarSym) {
        self.var_scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), sym);
    }

    fn declare_type(&mut self, name: &str, target: Target) {
        self.type_scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), target);
    }

    /// 预扫：把顶层 class/typedef（类型名）与顶层函数 / extern（变量名）灌进最外层
    /// 作用域。Lua 里顶层函数和全局是前向可见的，类型名同理，靠这一趟让「先用后声明」
    /// 的引用也连得上。局部变量不在此列 —— 它们只在声明之后可见，留给主遍历现声明
    fn hoist_top_level(&mut self, ast: &Tree<NodeSyntax<'_>>, module: &str, root: usize) {
        let mut types: Vec<(String, Span)> = Vec::new();
        let mut vars: Vec<(String, Span)> = Vec::new();
        each_node(ast, root, &mut |n| {
            let Some(prod) = get_production(ast, n) else {
                return;
            };
            match prod {
                Prod::ClassDecl | Prod::ClassDeclExtends | Prod::TypeDef
                    if is_chunk_top(ast, n) =>
                {
                    if let Some(nn) = ast.get_node(n).children.get(2).copied()
                        && let Some(name) = get_name(ast, nn)
                    {
                        types.push((name.to_string(), ast.span_of(nn)));
                    }
                }
                Prod::FuncDecl if is_chunk_top(ast, n) => {
                    // `function f`：funcname 折成 @FuncName，名字在它的 child0；
                    // `local function f`：NAME 直接在 child2
                    let children = &ast.get_node(n).children;
                    let name_node = if children.len() == 4 {
                        children.get(2).copied()
                    } else {
                        children
                            .get(1)
                            .copied()
                            .filter(|&c| get_production(ast, c) == Some(Prod::FuncName))
                            .and_then(|c| ast.get_node(c).children.first().copied())
                    };
                    if let Some(nn) = name_node
                        && let Some(name) = get_name(ast, nn)
                    {
                        vars.push((name.to_string(), ast.span_of(nn)));
                    }
                }
                Prod::Extern if is_chunk_top(ast, n) => {
                    if let Some(nn) = ast.get_node(n).children.get(1).copied()
                        && let Some(name) = get_name(ast, nn)
                    {
                        vars.push((name.to_string(), ast.span_of(nn)));
                    }
                }
                _ => {}
            }
        });
        for (name, span) in types {
            self.declare_type(
                &name,
                Target {
                    module: module.to_string(),
                    span,
                },
            );
        }
        for (name, span) in vars {
            self.declare_var(
                &name,
                VarSym {
                    span,
                    kind: TokenKind::Function,
                },
            );
        }
    }

    // ---- 分类：一个带标签的产生式怎么产出 token / 连线 ----

    fn classify(&mut self, ast: &Tree<NodeSyntax<'_>>, node: usize, prod: Prod) {
        let children = ast.get_node(node).children.clone();
        match prod {
            // 唯一查变量的地方：`prefixexp : NAME`
            Prod::VarRef => {
                if let Some(nn) = children.first().copied() {
                    self.use_var(ast, nn, node);
                }
            }
            // funcname 的基名。裸名字（parent 是 @FuncDecl）是被声明的全局函数；
            // 嵌在 @DottedName/@MethodName 里则是接收者，按变量读处理
            Prod::FuncName => {
                let Some(nn) = children.first().copied() else {
                    return;
                };
                let is_base_ref = ast
                    .get_node(node)
                    .parent
                    .map(|p| {
                        matches!(
                            get_production(ast, p),
                            Some(Prod::DottedName | Prod::MethodName)
                        )
                    })
                    .unwrap_or(false);
                if is_base_ref {
                    self.use_var(ast, nn, nn);
                } else if let Some(name) = get_name(ast, nn) {
                    let span = self.emit_node(ast, nn, TokenKind::Function);
                    // `function f` 是全局赋值，登记进最外层，全文件可见
                    self.var_scopes[0].insert(
                        name.to_string(),
                        VarSym {
                            span,
                            kind: TokenKind::Function,
                        },
                    );
                }
            }
            // 声明形式的名字：登记进作用域，jump 落到自己身上（不连线）
            Prod::DeclFirst => self.declare_at(ast, &children, 0, TokenKind::Variable),
            Prod::DeclRest => self.declare_at(ast, &children, 2, TokenKind::Variable),
            Prod::Param => self.declare_at(ast, &children, 0, TokenKind::Parameter),
            Prod::ForNum | Prod::ForNumStep => {
                self.declare_at(ast, &children, 1, TokenKind::Variable)
            }
            Prod::Extern => self.declare_at(ast, &children, 1, TokenKind::Variable),
            // 泛型形参：登进类型作用域，本类/本函数体内的 `T` 就连得回来
            Prod::TypeParam | Prod::TypeParamBound => {
                if let Some(nn) = children.first().copied()
                    && let Some(name) = get_name(ast, nn)
                {
                    let span = self.emit_node(ast, nn, TokenKind::TypeParameter);
                    self.declare_type(
                        name,
                        Target {
                            module: self.module.clone(),
                            span,
                        },
                    );
                }
            }
            // class / typedef 声明名：名字已在 hoist 里登记，这里只上色
            Prod::ClassDecl | Prod::ClassDeclExtends | Prod::TypeDef => {
                if let Some(nn) = children.get(2).copied() {
                    self.emit_node(ast, nn, TokenKind::Type);
                }
            }
            // 类型位的名字引用：连回声明（本文件或跨模块），连不上就只上色
            Prod::TypeName | Prod::GenericType => {
                if let Some(nn) = children.first().copied() {
                    let span = self.emit_node(ast, nn, TokenKind::Type);
                    if let Some(name) = get_name(ast, nn)
                        && let Some(t) = self.type_target(name)
                    {
                        self.link(span, t);
                    }
                }
            }
            // 字段 / 方法 / 属性访问：没有类型信息，只上色不跳转
            Prod::Dot | Prod::MethodCall => self.color_at(ast, &children, 2, kind_of_dot(prod)),
            Prod::DottedName => self.color_at(ast, &children, 2, TokenKind::Property),
            Prod::MethodName => self.color_at(ast, &children, 2, TokenKind::Method),
            Prod::MethodSig => self.color_at(ast, &children, 0, TokenKind::Method),
            Prod::FieldDecl | Prod::FieldNamed => {
                self.color_at(ast, &children, 0, TokenKind::Property)
            }
            Prod::ArgTypeNamed => self.color_at(ast, &children, 0, TokenKind::Parameter),
            // goto / label
            Prod::Goto | Prod::Label => self.color_at(ast, &children, 1, TokenKind::Label),
            // import：记下目标模块，具体每条 item 交给下面两支
            Prod::Import => {
                self.current_import = children
                    .get(5)
                    .copied()
                    .and_then(|n| get_string(ast, n))
                    .map(string_literal);
            }
            Prod::ImportItem => self.resolve_import(ast, &children, 0, None),
            Prod::ImportAlias => self.resolve_import(ast, &children, 0, Some(2)),
            _ => {}
        }
    }

    /// 变量读：`name_node` 是 NAME 节点，`ref_node` 是引用点（@VarRef 用它自己）。
    /// 查得到就按符号种类上色并连线；查不到又是写位（`x = 1` 的 x）就当全局定义登记
    fn use_var(&mut self, ast: &Tree<NodeSyntax<'_>>, name_node: usize, ref_node: usize) {
        let Some(name) = get_name(ast, name_node) else {
            return;
        };
        let span = ast.span_of(name_node);
        if let Some(sym) = self.resolve_var(name) {
            self.emit(span, sym.kind);
            self.link(
                span,
                Target {
                    module: self.module.clone(),
                    span: sym.span,
                },
            );
        } else {
            self.emit(span, TokenKind::Variable);
            // 没声明过的写位：第一次赋值就当它的定义点，后面的引用连到这儿
            if is_write(ast, ref_node) {
                self.var_scopes[0].insert(
                    name.to_string(),
                    VarSym {
                        span,
                        kind: TokenKind::Variable,
                    },
                );
            }
        }
    }

    /// 声明一个变量/形参：上色并登进当前作用域，jump 落自己身上
    fn declare_at(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        children: &[usize],
        i: usize,
        kind: TokenKind,
    ) {
        if let Some(nn) = children.get(i).copied()
            && let Some(name) = get_name(ast, nn)
        {
            let span = self.emit_node(ast, nn, kind);
            self.declare_var(name, VarSym { span, kind });
        }
    }

    /// 只上色、不登记也不连线（字段 / 方法 / label 这类没身份可查的名字）
    fn color_at(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        children: &[usize],
        i: usize,
        kind: TokenKind,
    ) {
        if let Some(nn) = children.get(i).copied() {
            self.emit_node(ast, nn, kind);
        }
    }

    /// 一条 import item：`orig_i` 是原名下标，`local_i` 是本地别名下标（None = 无别名）。
    /// 原名连回目标模块的定义；本地名（别名或原名自己）登进类型作用域，
    /// 于是文件里对它的类型引用能一路跳到别的文件
    fn resolve_import(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        children: &[usize],
        orig_i: usize,
        local_i: Option<usize>,
    ) {
        let Some(orig_node) = children.get(orig_i).copied() else {
            return;
        };
        let Some(orig) = get_name(ast, orig_node) else {
            return;
        };
        let orig_span = self.emit_node(ast, orig_node, TokenKind::Type);

        // 目标：当前 import 的模块里那个同名导出
        let target = self
            .current_import
            .as_ref()
            .and_then(|m| self.exports.get(m).map(|t| (m.clone(), t)))
            .and_then(|(m, t)| t.get(orig).map(|&span| Target { module: m, span }));

        if let Some(t) = &target {
            self.link(orig_span, t.clone());
        }

        // 本地名：有别名就是那个（也给它上色），没有就是原名自己
        let (local_name, _) = match local_i.and_then(|i| children.get(i).copied()) {
            Some(alias_node) => {
                let name = get_name(ast, alias_node);
                self.emit_node(ast, alias_node, TokenKind::Type);
                (name, alias_node)
            }
            None => (Some(orig), orig_node),
        };
        if let (Some(name), Some(t)) = (local_name, target) {
            self.declare_type(name, t);
        }
    }
}

impl DFS<()> for Analysis<'_> {
    fn enter(
        &mut self,
        _data: &mut (),
        ast: &Tree<NodeSyntax<'_>>,
        node: usize,
        _diags: &mut Vec<Diagnostic>,
    ) {
        match ast.get_node(node).get_data() {
            // 叶子 token：关键字 / 字面量 / 注释走这里，标点跳过。NAME 归各产生式管，
            // 在这里不碰（否则每个字段名都会被当普通标识符上色）
            NodeSyntax::Token(tok) => match tok {
                Token::RESERVED(_) => self.emit(ast.span_of(node), TokenKind::Keyword),
                Token::STRING(_) => self.emit(ast.span_of(node), TokenKind::String),
                Token::NUMERAL(_) => self.emit(ast.span_of(node), TokenKind::Number),
                _ => {}
            },
            NodeSyntax::Prod(Some(prod)) => {
                let prod = *prod;
                // 先开作用域再分类：本节点声明的名字（形参 / 泛型形参）要落在
                // 这一格里，不能漏到外层
                if opens_scope(prod) {
                    self.var_scopes.push(HashMap::new());
                    self.type_scopes.push(HashMap::new());
                }
                self.classify(ast, node, prod);
            }
            NodeSyntax::Prod(None) => {}
        }
    }

    fn leave(
        &mut self,
        _data: &mut (),
        ast: &Tree<NodeSyntax<'_>>,
        node: usize,
        _diags: &mut Vec<Diagnostic>,
    ) {
        if let NodeSyntax::Prod(Some(prod)) = ast.get_node(node).get_data() {
            let prod = *prod;
            if opens_scope(prod) {
                self.var_scopes.pop();
                self.type_scopes.pop();
            }
            if prod == Prod::Import {
                self.current_import = None;
            }
        }
    }
}

// ================== 纯 CST 形状查询（只读 ast） ==================

/// 这些产生式圈出一层新作用域：块、函数体 / 方法体、以及带泛型形参的
/// class / typedef 头。push/pop 成对，靠这一个判据保证配平
fn opens_scope(prod: Prod) -> bool {
    matches!(
        prod,
        Prod::Block
            | Prod::FuncBody
            | Prod::MethodDef
            | Prod::ClassDecl
            | Prod::ClassDeclExtends
            | Prod::TypeDef
    )
}

/// @Dot 的末名是属性，@MethodCall 的末名是方法
fn kind_of_dot(prod: Prod) -> TokenKind {
    match prod {
        Prod::MethodCall => TokenKind::Method,
        _ => TokenKind::Property,
    }
}

fn get_production(ast: &Tree<NodeSyntax<'_>>, node: usize) -> Option<Prod> {
    match ast.get_node(node).get_data() {
        NodeSyntax::Prod(prod) => *prod,
        NodeSyntax::Token(_) => None,
    }
}

fn get_name<'t>(ast: &'t Tree<NodeSyntax<'t>>, node: usize) -> Option<&'t str> {
    match ast.get_node(node).get_data() {
        NodeSyntax::Token(Token::NAME(name)) => Some(name),
        _ => None,
    }
}

fn get_string<'t>(ast: &'t Tree<NodeSyntax<'t>>, node: usize) -> Option<&'t str> {
    match ast.get_node(node).get_data() {
        NodeSyntax::Token(Token::STRING(s)) => Some(s),
        _ => None,
    }
}

/// 这个 @VarRef 是不是赋值目标：父节点是 @Var（varlist 里的裸名字写位）才算。
/// `a.b = 1` / `a[i] = 1` 的 @VarRef 挂在 @Dot / @Index 下，父不是 @Var，是读
fn is_write(ast: &Tree<NodeSyntax<'_>>, node: usize) -> bool {
    ast.get_node(node)
        .parent
        .map(|p| get_production(ast, p) == Some(Prod::Var))
        .unwrap_or(false)
}

/// 这条语句在不在 chunk 的直接语句列表里。顶层声明（导出的类型、顶层函数、
/// extern）只认这一层。`chunk : block` 折叠掉了，所以根就是那个 @Block
fn is_chunk_top(ast: &Tree<NodeSyntax<'_>>, node: usize) -> bool {
    let mut cur = node;
    while let Some(parent) = ast.get_node(cur).parent {
        match get_production(ast, parent) {
            Some(Prod::Block) => return ast.get_node(parent).parent.is_none(),
            Some(Prod::ListTail) => cur = parent,
            _ => return false,
        }
    }
    false
}

/// 前序遍历整棵树，对每个节点调用 `f`。hoist / index 这类不需要作用域的整树扫描用它
fn each_node(ast: &Tree<NodeSyntax<'_>>, node: usize, f: &mut impl FnMut(usize)) {
    f(node);
    for &child in &ast.get_node(node).children {
        each_node(ast, child, f);
    }
}
