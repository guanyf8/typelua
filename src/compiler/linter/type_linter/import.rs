//! Split out of type_linter.rs. Methods stay inherent on TypeLinter;
//! `use super::*;` pulls in the struct, helper types and imports.
use super::*;

impl TypeLinter {
    // ================== §11 跨模块 ==================
    //
    // import / pub。搬的只是「类型名 -> DeclId」这层绑定，纯编译期、擦除后
    // 一个字不剩。两条都【只许顶层】—— 文法表达不了，所以都在这里拦

    /// `import '{' importlist '}' IN STRING` —— 把别的模块 pub 出的类型引进
    /// 当前文件的类型命名空间。纯编译期：绑的只是类型名 -> DeclId，
    /// 擦除后一个字不剩。【只许顶层】文法表达不了，和 extern 一样归这里拦。
    ///
    /// @ImportAlias 的逐条改名也在这里一并处理（spine 把两种 item 都收上来），
    /// 所以 check_import_alias 无事
    pub(super) fn check_import(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) {
        let children = ast.get_node(node_index).children.clone();
        if !self.is_chunk_top(ast, node_index) {
            log.push(Logger {
                span: ast.span_of(node_index),
                msg: "import 只能写在文件顶层".to_string(),
            });
            return;
        }
        // 模块名在 children[5] 的 STRING 里，要去引号 / 解转义才是真正的名字
        let Some(&str_node) = children.get(5) else {
            return;
        };
        let module_str = match ast.get_node(str_node).get_data() {
            NodeSyntax::Token(Token::STRING(s)) => string_literal(s),
            _ => return,
        };
        let module_id = self.session.names.intern(&module_str);
        let Some(&list_node) = children.get(2) else {
            return;
        };
        for item in self.spine(ast, list_node) {
            // @ImportAlias : NAME AS NAME —— 原名 children[0]、本地名 children[2]；
            // @ImportItem  : NAME        —— 原名即本地名
            let is_alias = self.get_production(ast, item) == Some(Prod::ImportAlias);
            let Some(orig) = self.get_child_name(ast, item, 0) else {
                continue;
            };
            let local = if is_alias {
                let Some(l) = self.get_child_name(ast, item, 2) else {
                    continue;
                };
                l
            } else {
                orig
            };
            let orig_id = self.session.names.intern(orig);
            let span = ast.span_of(item);
            match self.session.lookup_export(module_id, orig_id) {
                // 查到导出：本文件已有同名类型→撞名，否则把本地名绑到那条 Decl 上。
                // 碰名得问 lookup_type：顶层类型是 prepare 里 hoist 进**文件级**作用域的，
                // import 的 declare_type 却落在上层 chunk 块那格，只看顶格的 declare_type 拓不到它
                Some(decl) => {
                    if self.lookup_type(local).is_some() {
                        log.push(Logger {
                            span,
                            msg: format!("{local} 已经声明过了"),
                        });
                    } else {
                        self.declare_type(local, TypeRef::Decl(decl));
                    }
                }
                // 查不到导出：模块里真有这类型只是没 pub，跟压根没这类型，报不同的错
                None => {
                    if self.session.module_has_type(module_id, orig_id) {
                        log.push(Logger {
                            span,
                            msg: format!("类型 {orig} 没有 pub，不能 import"),
                        });
                    } else {
                        log.push(Logger {
                            span,
                            msg: format!("模块 {module_str} 没有导出类型 {orig}"),
                        });
                    }
                }
            }
        }
    }

    /// 单条改名 import。所有 item（含 @ImportAlias）都由 check_import 扫 spine 时
    /// 一并处理了，这里无事—— 留个空钩子只为了分派表不缺一行
    pub(super) fn check_import_alias(
        &mut self,
        _ast: &Tree<NodeSyntax<'_>>,
        _node_index: usize,
        _log: &mut Vec<Logger>,
    ) {
    }

    /// `pub` —— 导出修饰，只贴在顶层 class / typedef 前。把被修饰的那个
    /// 类型名写进 Session::export；【只许顶层】文法表达不了，跟 extern 一样得在这里拦
    pub(super) fn check_pub(
        &mut self,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        log: &mut Vec<Logger>,
    ) {
        // @Pub 的父节点就是被修饰的 class / typedef 声明：文法只在这三条里
        // 开了 optpub 槽，所以父必是其一，NAME 恒在 children[2]
        let Some(decl_node) = ast.get_node(node_index).parent else {
            return;
        };
        if !self.is_chunk_top(ast, decl_node) {
            log.push(Logger {
                span: ast.span_of(node_index),
                msg: "pub 只能修饰顶层的 class / typedef".to_string(),
            });
            return;
        }
        let Some(&name_node) = ast.get_node(decl_node).children.get(2) else {
            return;
        };
        let Some(name) = self.get_name(ast, name_node) else {
            return;
        };
        let name_span = ast.span_of(name_node);
        let Some(decl) = self.hoisted_decl(ast, decl_node, name) else {
            return;
        };
        // 重复声明的第二条：名字归第一条所有，导出也只该按第一条。重名诊断
        // 已由 check_class_decl / check_type_def 报，这里静默跳过、不重复登记
        if self.is_dup_decl(decl, name_span) {
            return;
        }
        let name_id = self.session.names.intern(name);
        self.session.decls.get_mut(decl).exported = true;
        // 同模块同名重复导出（两个文件都 pub 了同名类型）：export 返回 false。
        // 同文件内的重名早被上面的 is_dup_decl 挡掉，走到这儿的都是跨文件撞名
        if !self.session.export(self.current_module, name_id, decl) {
            log.push(Logger {
                span: name_span,
                msg: format!("类型 {name} 已经被导出过了"),
            });
        }
    }
}
