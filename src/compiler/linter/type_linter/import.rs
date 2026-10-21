//! Split out of type_linter.rs. Methods stay inherent on TypeLinter;
//! `use super::*;` pulls in the struct, helper types and imports.
use super::*;

/// import 语句里拆出来的一条：本地名 / 原名 / 单条 item 的 span。
/// 只活在本文件的借用里，不往 Context 存
struct ImportItem<'a> {
    local: &'a str,
    orig: &'a str,
    span: Span,
}

impl TypeLinter {
    // ================== §11 跨模块 ==================
    //
    // import / pub。类型面搬的是「类型名 -> DeclId」，语句本身擦除后一个字不剩；
    // 但 class 的绑定要多记对面模块名与导出原名 —— 值位改写（`get`）靠它。
    // 两条都【只许顶层】—— 文法表达不了，所以都在这里拦

    /// 一条 import 的 item 列表。`@ImportAlias` 收起两种写法：
    /// 别名取 children[2] 的本地名，普通 item 原名即本地名
    fn import_items<'t>(
        &self,
        ast: &'t Tree<NodeSyntax<'t>>,
        node_index: usize,
    ) -> Vec<ImportItem<'t>> {
        let children = &ast.get_node(node_index).children;
        let Some(&list_node) = children.get(2) else {
            return Vec::new();
        };
        let mut items = Vec::new();
        for node in self.spine(ast, list_node) {
            let Some(orig) = self.get_child_name(ast, node, 0) else {
                continue;
            };
            let local = if self.get_production(ast, node) == Some(Prod::ImportAlias) {
                match self.get_child_name(ast, node, 2) {
                    Some(local) => local,
                    None => continue,
                }
            } else {
                orig
            };
            items.push(ImportItem {
                local,
                orig,
                span: ast.span_of(node),
            });
        }
        items
    }

    /// import 语句里的模块名（去引号 / 解转义）。`import '{' importlist '}' IN STRING`
    /// 的形状下 STRING 恒在 children[5]
    fn import_module(&self, ast: &Tree<NodeSyntax<'_>>, node_index: usize) -> Option<String> {
        let &str_node = ast.get_node(node_index).children.get(5)?;
        match ast.get_node(str_node).get_data() {
            NodeSyntax::Token(Token::STRING(s)) => Some(string_literal(s)),
            _ => None,
        }
    }

    /// BUILD 相位二的第一趟：把本文件的 import 全绑上。
    ///
    /// 必须晚于**所有**模块的 hoist_exports，所以不能和 `warm_up` 挤在一趟；
    /// 又必须早于 `hoist_class_bodies` —— 类体的 extends 可能就是 import 来的。
    /// 这趟不发诊断：真正的诊断留给主遍历的 check_import，输出顺序才跟源码一致
    pub(super) fn hoist_imports(&mut self, ctx: &mut Context, ast: &Tree<NodeSyntax<'_>>) {
        for stat in self.top_level_stats(ast) {
            if self.get_production(ast, stat) != Some(Prod::Import) {
                continue;
            }
            let Some(module) = self.import_module(ast, stat) else {
                continue;
            };
            let module_id = self.session.names.intern(&module);
            let statement_span = ast.span_of(stat);
            for item in self.import_items(ast, stat) {
                let orig_id = self.session.names.intern(item.orig);
                let Some(decl) = self.session.lookup_export(module_id, orig_id) else {
                    continue;
                };
                // 本文件已有同名类型：撞名的诊断归 check_import，这趟只负责不覆盖
                if self.lookup_type(ctx, item.local).is_some() {
                    continue;
                }
                self.declare_type(ctx, item.local, TypeRef::Decl(decl));
                ctx.imports.insert(
                    item.local.to_string(),
                    ImportBinding {
                        module: module.clone(),
                        class: item.orig.to_string(),
                        decl,
                        item_span: item.span,
                        statement_span,
                    },
                );
            }
        }
    }

    /// `import '{' importlist '}' IN STRING` —— 把别的模块 pub 出的类型引进
    /// 当前文件的类型命名空间。绑定已在 prepare 的 hoist_imports 里做完，
    /// 这里只补诊断；碰见同一条 item（同 span）就跳过，不报「已声明」
    pub(super) fn check_import(
        &mut self,
        ctx: &mut Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        diags: &mut Vec<Diagnostic>,
    ) {
        if !self.is_chunk_top(ast, node_index) {
            diags.push(Diagnostic {
                severity: DiagLevel::Error,
                span: ast.span_of(node_index),
                msg: "import 只能写在文件顶层".to_string(),
            });
            return;
        }
        let Some(module_str) = self.import_module(ast, node_index) else {
            return;
        };
        let module_id = self.session.names.intern(&module_str);
        let statement_span = ast.span_of(node_index);
        for item in self.import_items(ast, node_index) {
            // 同一条 item 已经由 hoist_imports 绑过：不再重复走一遍绑定/诊断
            if ctx
                .imports
                .get(item.local)
                .is_some_and(|bound| bound.item_span == item.span)
            {
                continue;
            }
            let orig_id = self.session.names.intern(item.orig);
            let span = item.span;
            match self.session.lookup_export(module_id, orig_id) {
                // 查到导出：本文件已有同名类型→撞名，否则把本地名绑到那条 Decl 上。
                // 碰名得问 lookup_type：顶层类型是 prepare 里 hoist 进**文件级**作用域的，
                // import 的 declare_type 却落在上层 chunk 块那格，只看顶格的 declare_type 拓不到它
                Some(decl) => {
                    if self.lookup_type(ctx, item.local).is_some() {
                        diags.push(Diagnostic {
                            severity: DiagLevel::Error,
                            span,
                            msg: format!("{} 已经声明过了", item.local),
                        });
                    } else {
                        self.declare_type(ctx, item.local, TypeRef::Decl(decl));
                        ctx.imports.insert(
                            item.local.to_string(),
                            ImportBinding {
                                module: module_str.clone(),
                                class: item.orig.to_string(),
                                decl,
                                item_span: span,
                                statement_span,
                            },
                        );
                    }
                }
                // 查不到导出：模块里真有这类型只是没 pub，跟压根没这类型，报不同的错
                None => {
                    if self.session.module_has_type(module_id, orig_id) {
                        diags.push(Diagnostic {
                            severity: DiagLevel::Error,
                            span,
                            msg: format!("类型 {} 没有 pub，不能 import", item.orig),
                        });
                    } else {
                        diags.push(Diagnostic {
                            severity: DiagLevel::Error,
                            span,
                            msg: format!("模块 {module_str} 没有导出类型 {}", item.orig),
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
        _diags: &mut Vec<Diagnostic>,
    ) {
    }

    /// warm_up 的导出登记趟：扫顶层 class / typedef，把带 `pub` 的那些写进
    /// Session::export。逻辑和 check_pub 完全一致（同一条 Decl、同一张导出表），
    /// 只是挪到跨文件 BUILD 趟里、且不发诊断 —— import 端 CHECK 时无论文件顺序
    /// 如何都查得到导出。重复导出 / 非顶层 pub 的诊断仍由主遍历的 check_pub 报
    /// （export 对同一条 Decl 幂等，这趟先登记了也不会让 check_pub 误判重复）
    pub(super) fn hoist_exports(&mut self, ctx: &Context, ast: &Tree<NodeSyntax<'_>>) {
        for stat in self.top_level_stats(ast) {
            if self.type_decl_is_class(ast, stat).is_none() {
                continue;
            }
            // optpub 恒在 children[0]：是 @Pub 节点才算导出，空槽折成 Prod(None)
            let Some(&pub_node) = ast.get_node(stat).children.first() else {
                continue;
            };
            if self.get_production(ast, pub_node) != Some(Prod::Pub) {
                continue;
            }
            // NAME 恒在 children[2]；hoisted_decl 顺着文件级作用域拿到 hoist 出的 DeclId
            let Some(name) = self.get_child_name(ast, stat, 2) else {
                continue;
            };
            let Some(decl) = self.hoisted_decl(ctx, ast, stat, name) else {
                continue;
            };
            let name_id = self.session.names.intern(name);
            self.session.decls.get_mut(decl).exported = true;
            // 返回值忽略：跨文件重复导出的诊断留给主遍历的 check_pub，这趟静默
            self.session.export(ctx.current_module, name_id, decl);
        }
    }

    /// `pub` —— 导出修饰，只贴在顶层 class / typedef 前。把被修饰的那个
    /// 类型名写进 Session::export；【只许顶层】文法表达不了，跟 extern 一样得在这里拦
    pub(super) fn check_pub(
        &mut self,
        ctx: &Context,
        ast: &Tree<NodeSyntax<'_>>,
        node_index: usize,
        diags: &mut Vec<Diagnostic>,
    ) {
        // @Pub 的父节点就是被修饰的 class / typedef 声明：文法只在这三条里
        // 开了 optpub 槽，所以父必是其一，NAME 恒在 children[2]
        let Some(decl_node) = ast.get_node(node_index).parent else {
            return;
        };
        if !self.is_chunk_top(ast, decl_node) {
            diags.push(Diagnostic {
                severity: DiagLevel::Error,
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
        let Some(decl) = self.hoisted_decl(ctx, ast, decl_node, name) else {
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
        if !self.session.export(ctx.current_module, name_id, decl) {
            diags.push(Diagnostic {
                severity: DiagLevel::Error,
                span: name_span,
                msg: format!("类型 {name} 已经被导出过了"),
            });
        }
    }
}
