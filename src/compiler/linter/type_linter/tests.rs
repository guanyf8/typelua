//! TypeLinter 的测试，从 type_linter.rs 里搬出来的两组：
//!   - `flow_tests`：手工按「驱动点会怎么调」的顺序去调 `flow_*`
//!   - `driver_tests`：源码 -> Parser -> LintDriver 的全程
//!
//! 两组都是 type_linter 的子模块，所以照样看得见它的私有项（`use super::super::*`）

/// 定值分析的内部单测：手工按「驱动点会怎么调」的顺序去调 `flow_*`。
/// 回滚、取交集、遮蔽、终结传播这几件事光看代码很容易想差一步，不实测不算数。
/// 驱动点自己走全程的测试在下面的 `driver_tests`
#[cfg(test)]
mod flow_tests {
    use super::super::*;
    use crate::parser::parser::Parser;

    /// 一个只有文件级作用域 + 文件帧的 linter，里面声明了 `local n:number`：
    /// number 容不下 nil、声明处又没给值，所以 `inited` 从 false 起步。
    /// 不需要 AST —— `flow_*` 全部只动内部状态，不读树
    fn linter() -> TypeLinter {
        let mut lint = TypeLinter::new();
        lint.scope_stack.push(Scope::new());
        lint.flow_reset();
        lint.declare_variable("n", TypeId::NUMBER, Span::default(), VarScope::Local, false);
        assert!(!lint.inited("n"));
        lint
    }

    impl TypeLinter {
        /// 测试用的查位快捷方式
        fn inited(&self, name: &str) -> bool {
            self.lookup_variable(name).expect("没声明过").inited
        }

        /// 跑一条分支：里面给 `n` 赋值，`terminated` 控制它走不走得出去
        fn branch_assigning_n(&mut self, terminated: bool) {
            self.flow_enter_branch();
            assert!(self.mark_inited("n"));
            if terminated {
                self.flow_terminate();
            }
            self.flow_leave_branch();
        }
    }

    /// `if c then n=1 else n=2 end` —— 两边都赋了值，交集非空。
    /// 中途那两句断言直接盯回滚：分支一退出就得恢复原状，否则
    /// `if c then n=1 end` 那种写法会跟着蒙混过关
    #[test]
    fn if_else_intersects_and_rolls_back() {
        let mut lint = linter();

        lint.flow_open();
        lint.branch_assigning_n(false);
        assert!(!lint.inited("n"), "分支退出必须回滚，合并之前外层看不见");
        lint.branch_assigning_n(false);
        assert!(!lint.inited("n"));
        lint.flow_close(JoinMode::Exhaustive);

        assert!(lint.inited("n"));
        assert!(
            lint.init_record.len() == 1,
            "合并结果要记回日志，外层才回滚得掉"
        );
    }

    /// `if c then n=1 end` —— 还有一条落空路径，交集必空。
    /// `while c do n=1 end` / `for` / funcbody 走的是同一支
    #[test]
    fn fallthrough_joins_nothing() {
        let mut lint = linter();

        lint.flow_open();
        lint.branch_assigning_n(false);
        lint.flow_close(JoinMode::Skippable);

        assert!(!lint.inited("n"));
        assert!(lint.init_record.is_empty());
    }

    /// `do n=1 end` / `repeat n=1 until c` —— 必定执行一次，原样应用
    #[test]
    fn always_taken_branch_applies() {
        let mut lint = linter();

        lint.flow_open();
        lint.branch_assigning_n(false);
        lint.flow_close(JoinMode::Always);

        assert!(lint.inited("n"));
    }

    /// `if c then n=1 else return end` —— 终结的分支不参与交集。
    /// 没这一条，guard 写法全部误报，比一刀切的「不许无初值」更招人烦
    #[test]
    fn terminated_branch_skipped() {
        let mut lint = linter();

        lint.flow_open();
        lint.branch_assigning_n(false);
        // else 分支什么都不赋，但 return 了
        lint.flow_enter_branch();
        lint.flow_terminate();
        lint.flow_leave_branch();
        lint.flow_close(JoinMode::Exhaustive);

        assert!(lint.inited("n"));
    }

    /// 两边都走不出来（`if c then return else error("x") end`）：
    /// 外层从这条语句起也到不了下一句
    #[test]
    fn all_terminated_propagates_outward() {
        let mut lint = linter();

        lint.flow_open();
        lint.branch_assigning_n(true);
        lint.branch_assigning_n(true);
        lint.flow_close(JoinMode::Exhaustive);

        assert!(lint.flow_terminated());
        assert!(!lint.inited("n"), "到不了的代码里那些赋值不能算数");
    }

    /// 分支里的内层同名变量不能泄到外层。这是 `InitFlip` 存作用域下标
    /// 而不是只存名字的缘由：按名字回滚 / 应用会打到被遮蔽的那一个
    #[test]
    fn inner_shadow_does_not_leak() {
        let mut lint = linter();

        lint.flow_open();
        lint.flow_enter_branch();
        // 分支自己那层作用域在 flow_enter_branch 之后才入栈
        lint.scope_stack.push(Scope::new());
        lint.declare_variable("n", TypeId::NUMBER, Span::default(), VarScope::Local, false);
        assert!(lint.mark_inited("n"));
        lint.scope_stack.pop();
        lint.flow_leave_branch();
        // 必定执行一次也不行：那次赋值根本不是给外层这个 n 赋的
        lint.flow_close(JoinMode::Always);

        assert!(!lint.inited("n"));
    }

    /// 函数体里的 `return` 不能终结外层：
    /// `local f = function() return end` 之后外层照样往下跑
    #[test]
    fn function_body_does_not_terminate_outer() {
        let mut lint = linter();

        lint.flow_open();
        lint.flow_enter_body();
        assert!(lint.mark_inited("n"));
        lint.flow_terminate();
        lint.flow_leave_branch();
        lint.flow_close(JoinMode::Skippable);

        assert!(!lint.flow_terminated());
        assert!(
            !lint.inited("n"),
            "不知道这个函数会不会被调，体内的赋值不能算数"
        );
    }

    /// `goto` 的降级到函数边界为止：嵌套闭包里一个 goto
    /// 不应该把整个文件的检查关掉
    #[test]
    fn untrust_stops_at_function_boundary() {
        let mut lint = linter();

        lint.flow_open();
        lint.flow_enter_body();
        lint.flow_enter_branch(); // 体内的一个 if 分支
        lint.flow_untrust();
        assert!(!lint.flow_trusted());
        lint.flow_leave_branch();
        assert!(!lint.flow_trusted(), "分支退出后这个函数体仍然不可信");
        lint.flow_leave_branch(); // 函数体
        lint.flow_close(JoinMode::Skippable);

        assert!(lint.flow_trusted(), "文件帧不该被它传染");
    }

    /// 判据在类型上、不在被调者的写法上：只要调用求出来是 `never` 就终结，
    /// 所以 `os.exit()` 这种字段读也算 —— 把位绑在变量上时它只能是漏报。
    /// 驱动点会在 `leave` 里问，那时调用的类型已经注册好，这里手工摆上
    #[test]
    fn noreturn_stat_reads_the_call_type() {
        for (src, ty, expect) in [
            ("error(\"x\")", TypeId::NEVER, true),
            ("os.exit()", TypeId::NEVER, true),
            ("print(\"x\")", TypeId::VOID, false),
            ("tostring(1)", TypeId::STRING, false),
        ] {
            let mut parser = Parser::new(src);
            let (ast, _) = parser.parse();
            let mut lint = TypeLinter::new();

            let stat = find_prod(ast, Prod::ExprStat).expect("这几句都是 @ExprStat");
            let call = ast.get_node(stat).children[0];
            lint.register_value(call, ty);
            assert_eq!(lint.is_noreturn_stat(ast, stat), expect, "{src}");
        }
    }

    /// `never` 能写（prelude 要在 .tlua 里声明 `-> never`），`unknown` 不能写
    /// —— 后者是「还没算出来」这个内部状态，能写就是关闭检查的后门。
    /// 顺带钉住预 intern 的顺序：NEVER 插在 VOID 前面，很容易错位
    #[test]
    fn never_is_writable_unknown_is_not() {
        let session = Session::new();
        assert_eq!(session.show(TypeId::NEVER).to_string(), "never");
        assert!(session.lookup_builtin_type("never").is_some());
        assert!(session.lookup_builtin_type("unknown").is_none());
        assert_eq!(
            session.type_arenas.get_type(TypeId::NEVER),
            Types::Trival(Trival::Never)
        );
        assert_eq!(
            session.type_arenas.get_type(TypeId::VOID),
            Types::Pack(ListId::EMPTY)
        );
    }

    /// 先序找第一个带这个标签的节点
    fn find_prod(ast: &Tree<NodeSyntax<'static>>, want: Prod) -> Option<usize> {
        let mut stack = vec![ast.get_root()?];
        while let Some(node) = stack.pop() {
            if let NodeSyntax::Prod(Some(p)) = ast.get_node(node).get_data() {
                if *p == want {
                    return Some(node);
                }
            }
            for &child in ast.get_node(node).children.iter().rev() {
                stack.push(child);
            }
        }
        None
    }
}

/// 驱动点的端到端测试：源码 -> Parser -> LintDriver -> 诊断。
///
/// 只断言文案里的关键词、不断言 span：前者才是 enter/leave 配对错了会变的
/// 东西。「一条不报」的用例和「报那一条」的一样重要 —— 帧开多了、作用域
/// 弹早了这类错先冲出来的那一面往往是误报
#[cfg(test)]
mod driver_tests {
    use super::super::*;
    use crate::parser::parser::Parser;

    /// 走真路径：不手工调 `enter`/`leave`，而是让 `LintDriver` 去走，
    /// 这样 `prepare` / `finish` 的配平自检也一并跑到
    fn lint(src: &str) -> Vec<String> {
        let mut parser = Parser::new(src);
        let (ast, _) = parser.parse();
        LintDriver::new()
            .init(TypeLinter::new())
            .run(ast, "main")
            .into_iter()
            .map(|entry| entry.msg)
            .collect()
    }

    /// 一条都不该报
    fn clean(src: &str) {
        let got = lint(src);
        assert!(got.is_empty(), "{src:?} 不该有诊断，实得 {got:?}");
    }

    /// 只该报一条，且文案里带 `want`
    fn only(src: &str, want: &str) {
        let got = lint(src);
        assert_eq!(got.len(), 1, "{src:?} 该只报一条，实得 {got:?}");
        assert!(
            got[0].contains(want),
            "{src:?} 该提到 {want:?}，实得 {got:?}"
        );
    }

    // ---- 声明与作用域 ----

    /// 没声明过的名字，读就报
    #[test]
    fn undeclared_read_reports() {
        only("local a = x", "未声明的变量 x");
    }

    /// 块级作用域真的弹得掉。这一条同时盯住「@Block 的 leave 里弹作用域」
    #[test]
    fn block_scope_ends() {
        only("do local x = 1 end\nlocal y = x", "未声明的变量 x");
    }

    /// 形参在作用域里、而且一律算已赋值
    #[test]
    fn param_is_declared_and_inited() {
        clean("local function f(p : number) local a = p end");
    }

    /// `local function f` 等于 `local f; f = function...`：名字得在体之前就在，
    /// 不然递归调自己会报未声明。两例都标了返回类型 —— 体里自引用而
    /// 又不标返回类型是另一条规矩，归 `recursive_func_needs_ret_annot`
    #[test]
    fn local_function_sees_itself() {
        clean("local function f() -> any return f end");
        clean("local function f(n : number) -> number return f(n) end");
    }

    /// 顶层函数抬升：b 的声明语句在后面，但 a 的体要等被调用才求值
    #[test]
    fn top_level_functions_are_hoisted() {
        clean("function a() b() end\nfunction b() end");
    }

    /// 抬升只抬名字不抬「已赋值」：立即执行位置读它照样要报
    #[test]
    fn hoisted_name_is_not_yet_assigned() {
        only("a()\nfunction a() end", "可能尚未赋值");
    }

    /// 全局的声明点只认顶层
    #[test]
    fn global_declaration_must_be_top_level() {
        only("do G = 1 end", "声明点只能在文件顶层");
    }

    /// 顶层赋值就是声明点，之后读它干净
    #[test]
    fn top_level_assign_declares_global() {
        clean("G = 1\nlocal a = G");
    }

    /// extern 声明处就算已赋值 —— 它真存不存在是宿主的事。
    /// 名字故意不拿标准库的：prelude 已经把 print / os 那一串占住了
    #[test]
    fn extern_is_declared_and_inited() {
        clean("extern host_hook : any\nlocal a = host_hook");
    }

    /// 「只许顶层」文法里表达不了，得 checker 拦
    #[test]
    fn extern_must_be_top_level() {
        only("do extern x : any end", "extern 只能写在文件顶层");
    }

    #[test]
    fn extern_rejects_duplicate() {
        only("extern x : any\nextern x : number", "已经声明过了");
    }

    /// 循环控制变量得落进体的作用域，且算已赋值
    #[test]
    fn loop_variables_are_bound() {
        clean("for i = 1, 10 do local a = i end");
        clean("for i = 1, 10, 2 do local a = i end");
        clean("extern it : any\nfor k, v in it do local a = k local b = v end");
    }

    /// 控制变量出了循环就不在了
    #[test]
    fn loop_variable_does_not_escape() {
        only("for i = 1, 10 do end\nlocal a = i", "未声明的变量 i");
    }

    /// `repeat local x = 1 until x == 1` 在 Lua 里合法：until 条件看得见体内的局部。
    /// 这就是体那格作用域要留给 @Repeat 去弹的缘由
    #[test]
    fn repeat_until_sees_body_locals() {
        clean("repeat local x = 1 until x == 1");
    }

    // ---- 定值分析 ----

    /// number 容不下 nil、声明处又没给值，所以读它要报
    #[test]
    fn declared_without_value_reports() {
        only("local n : number\nlocal m = n", "可能尚未赋值");
    }

    /// 容得下 nil 的类型声明出来就是诚实的
    #[test]
    fn nil_admitting_declaration_needs_no_value() {
        clean("local s : string | nil\nlocal m = s");
    }

    #[test]
    fn declaration_with_value_is_inited() {
        clean("local n : number = 1\nlocal m = n");
    }

    /// 两支都赋了值：交集非空，出了 if 就算已赋值
    #[test]
    fn both_branches_assign() {
        clean("local n : number\nif 1 then n = 1 else n = 2 end\nlocal m = n");
    }

    /// 只有 then 一支：落空路径存在，交集必空
    #[test]
    fn one_branch_is_not_enough() {
        only(
            "local n : number\nif 1 then n = 1 end\nlocal m = n",
            "可能尚未赋值",
        );
    }

    /// 走不出来的支不参与交集
    #[test]
    fn terminated_branch_is_excluded() {
        clean("local n : number\nif 1 then n = 1 else return end\nlocal m = n");
    }

    /// r2 的回本：守卫函数返回 `never`，调它的那一支也走不出来。
    /// 判据在类型上不在写法上 —— 这条一路验到调用的返回类型真的算出了 never
    #[test]
    fn never_call_ends_the_branch() {
        clean(
            "extern fail : function() -> never\n\
             local n : number\n\
             if 1 then n = 1 else fail() end\n\
             local m = n",
        );
    }

    /// 循环体可能一次也不跑：里面的赋值算不得数
    #[test]
    fn loop_body_may_not_run() {
        only(
            "local n : number\nwhile 1 do n = 1 end\nlocal m = n",
            "可能尚未赋值",
        );
    }

    /// 函数体里的赋值算不得数（不知道这个函数会不会被调），
    /// 但体内读上层变量也不能报 —— 体何时执行本就不知道
    #[test]
    fn upvalue_read_is_not_checked() {
        clean("local n : number\nlocal f = function() local m = n end\nn = 1");
    }

    /// 但函数自己的局部照查 —— 「按变量归属分」那一条的回本
    #[test]
    fn own_locals_are_checked_inside_function() {
        only(
            "local f = function() local n : number local m = n end",
            "可能尚未赋值",
        );
    }

    /// 向后跳让一遂前序合并失效，这一段的 inited 检查整个降级为放过
    #[test]
    fn goto_degrades_the_check() {
        clean("local n : number\n::again::\nlocal m = n");
    }

    /// 降级到函数边界为止：闭包里一个 label 不能把外层的检查关掉
    #[test]
    fn untrust_does_not_leak_out_of_closure() {
        only(
            "local f = function() ::again:: end\nlocal n : number\nlocal m = n",
            "可能尚未赋值",
        );
    }

    // ---- 可达性 ----

    #[test]
    fn statement_after_return_is_dead() {
        only("do return end\nlocal a = 1", "这条语句到不了");
    }

    /// 一个帧只报第一条：死代码是成段的，逐条报就是一屏
    #[test]
    fn dead_code_reports_once_per_frame() {
        only(
            "do return end\nlocal a = 1\nlocal b = 2\nlocal c = 3",
            "这条语句到不了",
        );
    }

    /// `break` 后面的语句也到不了。`stat : BREAK` 被折叠成了裸 token，
    /// 这一条盯的就是那一处认 token 的分支
    #[test]
    fn statement_after_break_is_dead() {
        only("while 1 do break local a = 1 end", "这条语句到不了");
    }

    /// 循环里的 `break` 终结不了循环外面
    #[test]
    fn break_does_not_kill_the_outer() {
        clean("while 1 do break end\nlocal a = 1");
    }

    /// label 是跳转落点，前一条 `goto` 终结不了它
    #[test]
    fn label_revives_the_frame() {
        clean("goto skip\n::skip::\nlocal a = 1");
    }

    /// 但 `goto` 后面、label 之前的那段确实到不了
    #[test]
    fn statement_after_goto_is_dead() {
        only("goto skip\nlocal a = 1\n::skip::", "这条语句到不了");
    }

    /// 调了个不返回的函数，后面也到不了
    #[test]
    fn statement_after_never_call_is_dead() {
        only(
            "extern fail : function() -> never\nfail()\nlocal a = 1",
            "这条语句到不了",
        );
    }

    /// 函数体里的 `return` 终结不了外层
    #[test]
    fn return_in_closure_does_not_kill_the_outer() {
        clean("local f = function() return end\nlocal a = 1");
    }

    // ---- never ----

    /// `never` 是内建类型名，各个类型位都写得出、解析得了。
    /// prelude 要靠它声明守卫函数，没这一步前面两条断流的结论无处可起
    #[test]
    fn never_is_a_writable_type_name() {
        clean("extern fail : function() -> never");
        clean("extern fail : function(string, number) -> never");
        // 参数位、泛型实参位、括号分组位：它就是个 type，不该有特例
        clean("extern f : function(never) -> number");
        clean("extern xs : array<never>");
        clean("extern g : function() -> (never)");
        // 变量位也写得出。它没有值，所以声明得出来但永远赋不上，
        // 于是读它报「可能尚未赋值」—— 这正是底类型该有的行为
        clean("local x : never");
        only("local x : never\nlocal y = x", "可能尚未赋值");
    }

    /// `unknown` 反过来——它是「还没算出来」这个内部状态，
    /// 能写就等于给用户一个关掉检查的后门
    #[test]
    fn unknown_is_not_a_type_name() {
        only("local x : unknown", "unknown");
    }

    // ---- Group A 结构解析（回归）----
    //
    // 这几条盯的是新落地的 resolve_paren / resolve_dot / func_sig_type：
    // 它们各自能不能把 `-> never` 的类型透过来，让 §9 的断流认得出。缺了任
    // 一环，调用点就算不出 never，后面那句「到不了」也就无从谈起

    /// resolve_dot：字段读能把 `-> never` 的函数类型取出来。调完之后到不了
    #[test]
    fn dot_field_never_call_is_dead() {
        only(
            "extern host : { exit : function() -> never }\nhost.exit()\nlocal a = 1",
            "这条语句到不了",
        );
    }

    /// resolve_paren：括号分组不把类型丢成 UNKNOWN，`(fail)()` 照样断流
    #[test]
    fn paren_preserves_never_call() {
        only(
            "extern fail : function() -> never\n(fail)()\nlocal a = 1",
            "这条语句到不了",
        );
    }

    /// func_sig_type（funcbody 位）：用户自己写的 `-> never` 函数，调它也终结当前帧
    #[test]
    fn user_func_body_never_ends_the_branch() {
        clean(
            "local n : number\n\
             local fail = function() -> never return end\n\
             if 1 then n = 1 else fail() end\n\
             local m = n",
        );
    }

    // ---- P0a 顶层类型名抬升 ----
    //
    // hoist_top_level_types 在遍历前把顶层 class / typedef 的名字先占进文件级
    // 作用域（空体），所以自指、互指、前向引用都能解析到同一个 DeclId，而不再
    // 报「未声明的类型」。填体是后面各 check_* 的事，这里只盯名字解析这一步

    /// 前向引用：typedef 写在 extern 之后，名字照样解析得到
    #[test]
    fn typedef_forward_ref_resolves() {
        clean("extern x : Count\ntypedef Count = number");
    }

    /// 自指：typedef 的体里引用自己，抬升后不再报未声明
    #[test]
    fn typedef_self_ref_resolves() {
        clean("typedef Node = { next : Node }");
    }

    /// 互指：两个 class 各自引用对方，两个名字都先占了名才都解析得到
    #[test]
    fn classes_mutually_reference() {
        clean("class A { other : B }\nclass B { other : A }");
    }

    /// class 也走同一条抬升：extern 在前、class 声明在后，前向引用照样通
    #[test]
    fn class_forward_ref_in_extern() {
        clean("extern w : Widget\nclass Widget { n : number }");
    }

    /// 只抬顶层：嵌在 do 里的 class 不是顶层声明点，外面引用不到它。
    /// 和 top_level_functions_are_hoisted 同一个尺度 —— 抬升只认 chunk 直属语句
    #[test]
    fn only_top_level_types_are_hoisted() {
        only(
            "do class Inner { n : number } end\nextern w : Inner",
            "未声明的类型 Inner",
        );
    }

    // ---- P1 class / typedef 名义体填充 ----
    //
    // check_class_decl(_extends) / check_type_def 把 P0a 占好的空体填上：字段、
    // extends、typedef 目标。这里盯填体过程直接报的诊断 —— 空体、重名字段、
    // 父类非 class、继承成环；泛型形参本身的声明不报（身份在 P0a 铸好，
    // 上界与实例化归各自的使用点管）

    /// 普通 class：有字段就不空，happy path 不该冒诊断
    #[test]
    fn class_with_fields_is_clean() {
        clean("class A { n : number, s : string }");
    }

    /// 非空体规则：没有父类的空 class 拒掉
    #[test]
    fn empty_class_without_extends_is_rejected() {
        only("class A {}", "不能为空");
    }

    /// 带 extends 的空体合法：形状从父类继承，不受非空规则约束
    #[test]
    fn empty_class_with_extends_is_allowed() {
        clean("class A { n : number }\nclass B : A {}");
    }

    /// 同名字段是笔误，报在重复那一项上
    #[test]
    fn duplicate_field_reports() {
        only("class A { x : number, x : string }", "字段 x 重复声明");
    }

    /// extends 只认 class：继承一个 typedef 要报
    #[test]
    fn extends_non_class_reports() {
        only(
            "typedef T = number\nclass B : T { n : number }",
            "只能继承 class",
        );
    }

    /// 继承成环：闭合那条声明（`class B : A`）查得出来
    #[test]
    fn extends_cycle_reports() {
        only(
            "class A : B { x : number }\nclass B : A { y : number }",
            "继承出现环",
        );
    }

    /// 多级继承链合法，不该被误判成环
    #[test]
    fn multilevel_extends_is_clean() {
        clean("class A { x : number }\nclass B : A { y : number }\nclass C : B { z : number }");
    }

    /// 父类写在后面：两条都已抬升，前向继承照样解析得到
    #[test]
    fn extends_forward_ref_is_clean() {
        clean("class B : A { y : number }\nclass A { x : number }");
    }

    /// 直接继承自己是环
    #[test]
    fn self_extends_reports_cycle() {
        only("class A : A {}", "继承出现环");
    }

    /// 三节点环：只在闭合那条（最后声明的 C）报一次
    #[test]
    fn three_node_extends_cycle_reports_once() {
        only(
            "class A : B { x : number }\nclass B : C { y : number }\nclass C : A { z : number }",
            "继承出现环",
        );
    }

    /// 名义继承只认直接的 class 名：继承一个「别名到 class 的 typedef」仍拒
    #[test]
    fn extends_typedef_alias_of_class_is_rejected() {
        only(
            "class A { n : number }\ntypedef T = A\nclass B : T {}",
            "只能继承 class",
        );
    }

    /// 内建容器不是 class，不能被继承
    #[test]
    fn extends_builtin_container_is_rejected() {
        only("class B : array<number> { n : number }", "只能继承 class");
    }

    /// 泛型 typedef 的形参没被体引用时照样干净：`T` 进了作用域但目标是 number、
    /// 用不上，既不报未声明也不报别的
    #[test]
    fn generic_typedef_is_deferred_silently() {
        clean("typedef Id<T> = number");
    }

    /// 顶层同名声明只有第一条算数：后来的同名 class 报重复，且不覆盖第一条的体
    #[test]
    fn duplicate_class_name_reports() {
        only(
            "class A { x : number }\nclass A { y : number }",
            "类型名 A 重复声明",
        );
    }

    /// class 撞上已声明的同名 typedef，也算重复声明
    #[test]
    fn class_typedef_name_clash_reports() {
        only(
            "typedef A = number\nclass A { x : number }",
            "类型名 A 重复声明",
        );
    }

    // ---- P1 可赋值性：标注初值 / 再赋值的类型核对 ----
    //
    // check_local_decl_init / check_assign 把 assignable 接上：有标注的初值、
    // 以及给已声明变量的再赋值，值都得塞得进目标类型。无标注不查 —— 那是把初值
    // 的类型直接当声明类型，谈不上「塞不塞得进」

    /// 标注和初值对得上：不报
    #[test]
    fn annotated_init_match_is_clean() {
        clean("local x : number = 1");
    }

    /// 标注和初值对不上：报
    #[test]
    fn annotated_init_mismatch_reports() {
        only("local x : number = \"s\"", "不能把");
    }

    /// 没标注就不谈可赋值性：初值的类型直接当声明类型
    #[test]
    fn unannotated_init_is_clean() {
        clean("local x = \"s\"");
    }

    /// 给已声明变量再赋值，值对得上声明类型：不报
    #[test]
    fn reassign_match_is_clean() {
        clean("local x : number = 1\nx = 2");
    }

    /// 再赋值对不上声明类型：报
    #[test]
    fn reassign_mismatch_reports() {
        only("local x : number = 1\nx = \"s\"", "声明为");
    }

    // ---- P2 收口：字段覆盖 ----
    //
    // 名义继承下，子类不许声明一个父类（含更上层）已有的同名字段。收口在
    // finish 里做：那时本文件所有 class 体都填好了，前向继承的父类也在

    /// 直接父类里有同名字段：报
    #[test]
    fn field_override_reports() {
        only(
            "class A { n : number }\nclass B : A { n : string }",
            "覆盖了继承来的同名字段",
        );
    }

    /// 同名字段藏在更上层的祖先里，沿 extends 链也得查出来
    #[test]
    fn field_override_through_chain_reports() {
        only(
            "class A { n : number }\nclass B : A {}\nclass C : B { n : string }",
            "覆盖了继承来的同名字段",
        );
    }

    /// 父类前向声明（写在子类后面）：finish 后扫才认得出这次覆盖
    #[test]
    fn field_override_forward_parent_reports() {
        only(
            "class B : A { n : string }\nclass A { n : number }",
            "覆盖了继承来的同名字段",
        );
    }

    /// 子类添的是父类没有的字段：不算覆盖，不报
    #[test]
    fn distinct_child_fields_are_clean() {
        clean("class A { n : number }\nclass B : A { m : string }");
    }

    // ---- 同层局部重复声明 ----
    //
    // Lua 允许同层遮蔽（`local x; local x`），TypeLua 不允许；跨层遮蔽仍然合法

    /// 同一层里再 local 一个同名的：报
    #[test]
    fn same_scope_local_redecl_reports() {
        only("local x = 1\nlocal x = 2", "重复声明");
    }

    /// 内层 block 盖外层同名：跨层遮蔽，合法
    #[test]
    fn shadowing_in_inner_scope_is_clean() {
        clean("local x = 1\ndo local x = 2 end");
    }

    /// `local function` 同层重名，走的是 enter 里的声明点，一样拦
    #[test]
    fn same_scope_local_function_redecl_reports() {
        only("local function f() end\nlocal function f() end", "重复声明");
    }

    /// 一条 `local a, a = 1, 2` 里自己撞自己：第二个名字登记时就该报
    #[test]
    fn same_stmt_repeated_name_reports() {
        only("local a, a = 1, 2", "重复声明");
    }

    // ---- P3 方法：methodsig 装成函数字段 + self 默认取本类 ----
    //
    // 方法就是持有函数值的字段，`a:f(x)` == `a.f(a, x)`。methodsig 产出普通 Func，
    // 收进 record；self 显式写出，不标注时类型默认为接收者那个类。
    //
    // 方法必带函数体（文法删掉了「只给签名」那条候选式），所以下面每一条
    // 类体里的方法都带着体

    /// 方法字段带着真正的返回类型：`a:greet(..)` 的结果是 string，塞进标注
    /// number 的 local 就该报 —— 证明方法字段不再是 UNKNOWN
    #[test]
    fn method_return_type_flows_to_call_site() {
        only(
            "class A { greet(self, msg:string) -> string return msg end }\nlocal function run(a : A) local n : number = a:greet(\"x\") end",
            "不能把",
        );
    }

    /// 返回类型对得上就干净：显式 self:A、方法返回 number、接给 number 的 local
    #[test]
    fn method_call_matching_return_is_clean() {
        clean(
            "class A { n : number, get(self:A) -> number return self.n end }\nlocal function run(a : A) local n : number = a:get() end",
        );
    }

    /// 类体内不标注的 self 默认为本类：方法体里把 self 接给标注 string 的 local
    /// 就报—— A 塞不进 string。self 若还是 UNKNOWN，这条就不报了。这里直接拿 self
    /// 本身赋值、不走字段访问，单测 self 的默认类型这一件事
    #[test]
    fn unannotated_self_defaults_to_enclosing_class() {
        only(
            "class A { n : number, bad(self) local x : string = self end }",
            "不能把",
        );
    }

    /// 内联方法体里的 `self.n` 能查到本类字段：字段在 prepare 的 hoist_class_bodies
    /// 里就填好了，不必等 class 的 leave。n 是 number，接给标注 string 的 local 就报
    #[test]
    fn self_field_access_resolves_inside_inline_method() {
        only(
            "class A { n : number, bad(self) local x : string = self.n end }",
            "不能把",
        );
    }

    /// 类体外方法重定义 `function A:m()`：':' 形式的 self 隐式，其类型是接收者 A。
    /// 体里 `self.n`（number）接给标注 string 的 local 就报 —— 证明隐式 self 绑上了。
    /// 类体里得先声明同名方法，不然先报的是「没有声明方法」
    #[test]
    fn external_method_binds_implicit_self() {
        only(
            "class A { n : number, bad(self) -> () end }\nfunction A:bad() -> () local x : string = self.n end",
            "不能把",
        );
    }

    /// 隐式 self 的字段类型对得上就干净：self.n 是 number，原样 return 给 number 返回位
    #[test]
    fn external_method_self_field_matching_is_clean() {
        clean(
            "class A { n : number, get(self) -> number return self.n end }\nfunction A:get() -> number return self.n end",
        );
    }

    // ---- 方法 vs 函数变量字段：两者不互为糖 ----
    //
    // `m(self) … end` 是**方法**：不参与 `A{…}` 构造，只有它能被类体外的
    // `function A:m` / `function A.m` 重定义。`m : function(…)` 是**函数变量字段**：
    // 和 string / number 一样的普通字段，没默认值就得每处构造都给，且不能用
    // `function` 去定义

    /// 类体里没声明过的名字，类体外不能凭空挂上去
    #[test]
    fn external_method_needs_in_body_declaration() {
        only(
            "class A { n : number }\nfunction A:m() -> () end",
            "没有声明方法 m",
        );
    }

    /// 函数变量字段不是方法，不许用 `function` 定义
    #[test]
    fn external_function_cannot_define_variable_field() {
        only(
            "class A { m : function(a:A) -> () }\nfunction A:m() -> () end",
            "是函数变量字段而不是方法",
        );
    }

    /// ':' 形式的签名得和声明对上（合成的 self 在形参表头）：形参类型不同就报
    #[test]
    fn external_method_signature_must_match() {
        only(
            "class A { m(self, x:number) -> () end }\nfunction A:m(x:string) -> () end",
            "和 class A 里的声明不一致",
        );
    }

    /// 点形式 `function A.m(self, …)`：Lua 5.3 里 '.' 不注入接收者，self 是写出来的
    /// 普通形参，所以原样和声明比。不标注的 self 同样默认成本类，所以这条干净
    #[test]
    fn external_dot_method_compares_written_self() {
        clean("class A { m(self, x:number) -> () end }\nfunction A.m(self, x:number) -> () end");
    }

    /// 外层点形式方法的接收者只属于它自己的 funcname / funcbody。嵌套函数的 self
    /// 没有接收者，保持 UNKNOWN；若错误继承了 A，这里的 string 标注就会报错
    #[test]
    fn external_dot_receiver_does_not_leak_into_nested_function() {
        clean(
            "class A { m(self) -> () end }\nfunction A.m(self) -> () local f = function(self) local s:string = self end end",
        );
    }

    /// 冒号形式的隐式 self 也只属于紧邻的 funcbody，不能泄漏给下一条普通函数
    #[test]
    fn external_colon_receiver_does_not_leak_to_next_function() {
        only(
            "class A { m(self) -> () end }\nfunction A:m() -> () end\nfunction plain() -> () local x = self end",
            "未声明的变量 self",
        );
    }

    /// 点形式不合成 self：声明的第一个形参是 self，定义里漏写就对不上
    #[test]
    fn external_dot_method_without_self_reports() {
        only(
            "class A { m(self, x:number) -> () end }\nfunction A.m(x:number) -> () end",
            "和 class A 里的声明不一致",
        );
    }

    /// 无 self 的静态方法：点形式原样比，所以它自然就支持
    #[test]
    fn external_dot_static_method_is_clean() {
        clean(
            "class A { n:number, make(x:number) -> number return x end }\nfunction A.make(x:number) -> number return x end",
        );
    }

    /// 类名不是变量：`A.m = …` 无处可写
    #[test]
    fn assign_to_class_name_field_reports() {
        only(
            "class A { m(self) -> () end }\nA.m = function() end",
            "是类名而不是变量",
        );
    }

    /// 方法不能被赋值整体覆盖 —— 能逐实例换的就不是方法而是函数变量字段
    #[test]
    fn assign_to_method_field_reports() {
        only(
            "class A { m(self) -> () end }\nlocal function run(a : A) a.m = function() end end",
            "是方法，不能赋值覆盖",
        );
    }

    /// 函数变量字段反过来：整体赋值合法
    #[test]
    fn assign_to_function_variable_field_is_clean() {
        clean(
            "class A { m : function() -> () = function() end }\nlocal function run(a : A) a.m = function() end end",
        );
    }

    // ---- 类型位的匿名 record 不能定义方法 ----
    //
    // `basictype : '{' classfieldlist '}'` 和 classbody 共用 classfieldlist，
    // 所以文法上写得出来；可那个函数体归谁、什么时候查都没有答案

    /// 类型位的 record 里带体的方法：报
    #[test]
    fn record_type_rejects_method_definition() {
        only(
            "typedef T = { m(self) -> () end }",
            "类型位的 record 不能定义方法",
        );
    }

    /// 写成函数变量字段就行
    #[test]
    fn record_type_function_field_is_clean() {
        clean("typedef T = { m : function() -> () }");
    }

    // ---- `A{…}` 构造 ----
    //
    // class 没有构造器，`A{…}` 就是逐字段核对一张表。不走 assignable：
    // class 是名义类型，`assignable(record, Ref A)` 按设计恒 false

    /// 没默认值的字段每处构造都得给
    #[test]
    fn construction_missing_required_field_reports() {
        only("class A { n : number }\nlocal a = A{}", "缺少字段 n");
    }

    /// 有默认值的可省
    #[test]
    fn construction_omitting_defaulted_field_is_clean() {
        clean("class A { n : number = 1 }\nlocal a = A{}");
    }

    /// 给了个不存在的字段
    #[test]
    fn construction_unknown_field_reports() {
        only(
            "class A { n : number }\nlocal a = A{n = 1, nope = 2}",
            "不是 A 的字段",
        );
    }

    /// 字段值的类型得塞得进声明位
    #[test]
    fn construction_field_type_mismatch_reports() {
        only(
            "class A { n : number }\nlocal a = A{n = \"s\"}",
            "赋给字段 n",
        );
    }

    /// 方法不参与构造：既不必给，也不许给
    #[test]
    fn construction_cannot_supply_method_reports() {
        only(
            "class A { m(self) -> () end }\nlocal a = A{m = function() end}",
            "是方法",
        );
    }

    /// 只有方法的类：什么都不用给
    #[test]
    fn construction_method_only_class_is_clean() {
        clean("class A { m(self) -> () end }\nlocal a = A{}");
    }

    /// 继承来的无默认值字段也得给
    #[test]
    fn construction_inherited_required_field_reports() {
        only(
            "class A { n : number }\nclass B : A { m : number = 0 }\nlocal b = B{}",
            "缺少字段 n",
        );
    }

    /// 继承来的字段给得上
    #[test]
    fn construction_supplying_inherited_field_is_clean() {
        clean("class A { n : number }\nclass B : A { m : number = 0 }\nlocal b = B{n = 1}");
    }

    /// 位置元素给不出字段名
    #[test]
    fn construction_positional_element_reports() {
        only(
            "class A { n : number = 0 }\nlocal a = A{1}",
            "只能用 字段名 = 值",
        );
    }

    /// 类名不是函数，只有表形式算构造
    #[test]
    fn calling_class_name_like_function_reports() {
        only(
            "class A { n : number = 0 }\nlocal a = A(1)",
            "只能用 A{…} 构造",
        );
    }

    /// 同名局部变量遮蔽类名：`A(1)` 就回到普通调用
    #[test]
    fn local_variable_shadows_class_name() {
        clean("class A { n : number = 0 }\nlocal A = function(x:number) end\nA(1)");
    }

    /// 构造的结果就是那个类：接给标注本类的 local 干净
    #[test]
    fn construction_result_is_the_class() {
        clean("class A { n : number = 0 }\nlocal a : A = A{}");
    }

    /// ……接给别的类就报
    #[test]
    fn construction_result_is_nominal() {
        only(
            "class A { n : number = 0 }\nclass B { n : number = 0 }\nlocal a : B = A{}",
            "不能把",
        );
    }

    /// 类名写在值位不再报「未声明的变量」：静态方法就是这么调的
    #[test]
    fn class_name_in_value_position_is_not_undeclared() {
        clean("class A { n:number, make(x:number) -> number return x end }\nlocal v = A.make(1)");
    }

    /// 方法的形参不外泄：`p` 是方法 `f` 的形参，类体外读它该报未声明
    #[test]
    fn method_params_do_not_leak_out_of_class() {
        only(
            "class A { n : number, f(self, p:number) end }\nlocal y = p",
            "未声明的变量 p",
        );
    }

    // ---- 字段默认值：`NAME ':' type '=' exp` 的默认值得塞得进标注位 ----
    //
    // check_field_decl 复用 assignable：标注是权威类型，默认值只是初值。
    // methodsig 形式没有 `= exp`，函数类型的默认值要写成普通字段 `f : function(..) -> .. = ..`

    /// 标量字段的默认值类型不符就报：标注 number、默认给字符串，string 塞不进 number
    #[test]
    fn field_default_type_mismatch_reports() {
        only("class A { m : number = \"x\" }", "不能把");
    }

    /// 默认值对得上就干净：标注 number、默认给 number
    #[test]
    fn field_default_matching_type_is_clean() {
        clean("class A { m : number = 5 }");
    }

    /// 函数字段当普通字段带默认值：标注是函数类型、默认却给了个标量，塞不进就报
    #[test]
    fn function_field_default_non_function_reports() {
        only("class A { act : function() -> number = 5 }", "不能把");
    }

    /// 无标注的默认值形式 `NAME '=' exp` 不参与检查：类型直接拿默认值推，
    /// 谈不上塞不塞得进。check_field_decl 该在 annotated 判定处提前返回，不误报
    #[test]
    fn unannotated_field_default_is_not_checked() {
        clean("class A { m = 5 }");
    }

    /// 函数字段默认给一个形状相符的函数字面量：标注 `function()` 与默认
    /// `function() end` 都是 `function() -> ()`，对得上就干净。这条一并钓住
    /// func_sig_type（functype 位）的参数收集：标注若还多出个幽灵 unknown 形参就会误报
    #[test]
    fn function_field_default_matching_closure_is_clean() {
        clean("class A { act : function() = function() end }");
    }

    // ---- 写位：标注只贴裸名、形状一次定死 ----
    //
    // check_write_target / check_field_write。README 把标注定成声明的一部分，
    // 而字段的声明只在 class 体 / record 类型里，所以写位既不能带标注、
    // 也不能长出新字段；table<K,V> 是例外 —— 它的形状就是「任意个 K 键」

    /// 字段写位不能带标注
    #[test]
    fn annotation_on_field_write_reports() {
        only(
            "typedef M = { NAME : string }\nlocal m : M = { NAME = \"a\" }\nm.NAME:string = \"demo\"",
            "只能贴在裸变量名上",
        );
    }

    /// 元素写位也不能带标注
    #[test]
    fn annotation_on_index_write_reports() {
        only(
            "local t : array<number> = {}\nt[1]:number = 2",
            "只能贴在裸变量名上",
        );
    }

    /// class 实例上新增字段：形状一次定死，运行时不开这条路
    #[test]
    fn writing_new_field_on_class_reports() {
        only(
            "class A { n : number = 0 }\nlocal a : A = A{}\na.extra = 1",
            "没有字段 extra",
        );
    }

    /// record 上也一样
    #[test]
    fn writing_new_field_on_record_reports() {
        only(
            "local m : { NAME : string } = { NAME = \"a\" }\nm.other = 1",
            "没有字段 other",
        );
    }

    /// 已有字段写对类型就干净，写错类型报
    #[test]
    fn writing_existing_field_checks_type() {
        clean("class A { n : number = 0 }\nlocal a : A = A{}\na.n = 2");
        only(
            "class A { n : number = 0 }\nlocal a : A = A{}\na.n = \"x\"",
            "字段 n",
        );
    }

    /// `table<K,V>` 的形状就是「任意个 K 键」，按名字写新键不算长新字段；
    /// 值的类型还是得对
    #[test]
    fn writing_new_key_on_map_is_allowed() {
        clean("local t : table<string, number> = {}\nt.k = 1");
        only("local t : table<string, number> = {}\nt.k = \"x\"", "表值");
    }

    /// 交类型里那半个 table 同理：已有字段走 record 那半，其它名字走容器那半。
    /// 一并钉住别名透明：owner 是个 typedef 引用，不展开就认不出里面那个 table
    #[test]
    fn intersect_with_map_admits_new_keys() {
        clean(
            "typedef M = { VERSION : number } & table<string, number>\nextern m : M\nm.VERSION = 2\nm.other = 3",
        );
    }

    /// 元素写位的值也得塞得进元素类型
    #[test]
    fn element_write_checks_value_type() {
        only("local t : array<number> = {}\nt[1] = \"x\"", "元素");
    }

    /// 赋值目标不能是调用或括号表达式
    #[test]
    fn call_is_not_an_lvalue() {
        only("local function f() end\nf() = 1", "赋值目标只能是变量");
    }

    // ---- 调用点：个数、逐位类型、方法存不存在 ----
    //
    // check_call_shape。少给得看缺的那几位收不收 nil —— Lua 里没传的形参
    // 就是 nil，所以 `p : number|nil` 少传合法；末位实参摊得开时个数算不出来，
    // 整条个数检查跳过

    /// 实参多了 / 少了都报，刚好就干净
    #[test]
    fn call_arity_is_checked() {
        clean("local function f(a : number, b : string) end\nf(1, \"x\")");
        only(
            "local function f(a : number, b : string) end\nf(1)",
            "实参个数不对",
        );
        only("local function f(a : number) end\nf(1, 2)", "实参个数不对");
    }

    /// 逐位类型核对
    #[test]
    fn call_argument_types_are_checked() {
        only("local function f(a : number) end\nf(\"x\")", "第 1 个实参");
    }

    /// 收得下 nil 的形参可以不传：没传就是 nil，这是 Lua 的语义而不是宽容
    #[test]
    fn omitting_a_nilable_param_is_fine() {
        clean("local function f(a : number, b : string|nil) end\nf(1)");
    }

    /// vararg 形参表收得下任意多的实参，但类型还是得对
    #[test]
    fn vararg_params_take_any_count() {
        clean("local function f(a : number, ...number) end\nf(1, 2, 3)");
        only(
            "local function f(a : number, ...number) end\nf(1, \"x\")",
            "第 2 个实参",
        );
    }

    /// 只有一格 vararg 的形参表：单孩子折叠后那格自己就得是「零定长 + 变长」，
    /// 折成裸类型会被当成一个定长形参、`f()` 和 `f(1,2)` 都要挨报
    #[test]
    fn lone_vararg_params_take_any_count() {
        clean("local function f(...number) end\nf()\nf(1, 2, 3)");
        only("local function f(...number) end\nf(\"x\")", "第 1 个实参");
        // 不带标注的 `...` 收什么都行
        clean("local function f(...) end\nf()\nf(1, \"x\", true)");
    }

    /// 末位实参是个摊得开的多值：个数算不出来，不能因此误报
    #[test]
    fn spread_call_skips_arity() {
        clean(
            "local function g() -> (...number) return 1 end\n\
             local function f(a : number, b : number) end\n\
             f(g())",
        );
    }

    /// 方法调用：':' 形式把接收者当第一个实参，所以 self 不算在实参里
    #[test]
    fn method_call_counts_the_receiver_as_self() {
        clean(
            "class A { n : number = 0, add(self, d : number) -> number return d end }\n\
             local a : A = A{}\n\
             local v : number = a:add(1)",
        );
        only(
            "class A { n : number = 0, add(self, d : number) -> number return d end }\n\
             local a : A = A{}\n\
             local v : number = a:add()",
            "实参个数不对",
        );
    }

    /// 调一个不存在的方法
    #[test]
    fn calling_a_missing_method_reports() {
        only(
            "class A { n : number = 0 }\nlocal a : A = A{}\na:ghost()",
            "没有方法 ghost",
        );
    }

    /// 拿标量当函数调
    #[test]
    fn calling_a_scalar_reports() {
        only("local n : number = 1\nn()", "不能调用");
    }

    /// 泛型函数的实参位是形参类型，拿不准就放过：个数还是要数对
    #[test]
    fn generic_call_checks_arity_not_types() {
        clean("local function id<T>(x : T) -> T return x end\nlocal v = id(1)");
        only(
            "local function id<T>(x : T) -> T return x end\nlocal v = id()",
            "实参个数不对",
        );
    }

    // ---- 读位：字段得真存在 ----
    //
    // resolve_dot。形状一次定死，所以读一个不存在的字段永远只能拿到 nil，
    // 那不是「还没算出来」而是写错了；拿不准形状的（any / unknown / 泛型形参）
    // 一律放过，否则在类型信息还不全的地方会刷一屏误报

    /// class 上读不存在的字段
    #[test]
    fn reading_missing_field_on_class_reports() {
        only(
            "class A { n : number = 0 }\nlocal a : A = A{}\nlocal x = a.ghost",
            "没有字段 ghost",
        );
    }

    /// record 上同理
    #[test]
    fn reading_missing_field_on_record_reports() {
        only(
            "local m : { NAME : string } = { NAME = \"a\" }\nlocal v = m.VERSION",
            "没有字段 VERSION",
        );
    }

    /// 存在的字段读出正确类型：接给 number 位干净、接给 string 位报
    #[test]
    fn reading_existing_field_gives_its_type() {
        clean("class A { n : number = 0 }\nlocal a : A = A{}\nlocal v : number = a.n");
        only(
            "class A { n : number = 0 }\nlocal a : A = A{}\nlocal v : string = a.n",
            "不能把",
        );
    }

    /// `table<K,V>` 按名字读就是读那一项，不算「没这个字段」
    #[test]
    fn reading_any_key_on_map_is_allowed() {
        clean("local t : table<string, number> = {}\nlocal v : number = t.k");
    }

    /// any 上随便读：渐进类型的口子，不报
    #[test]
    fn reading_field_on_any_is_clean() {
        clean("extern x : any\nlocal v = x.whatever");
    }

    /// 别名透明：`typedef Xs = array<number>` 上的下标和直接写 array 一样。
    /// resolve_index 不先展开别名就会把它当成非容器，静静给个 ANY
    #[test]
    fn index_through_a_typedef_alias_keeps_the_element_type() {
        clean("typedef Xs = array<number>\nextern xs : Xs\nlocal v : number = xs[1]");
        only(
            "typedef Xs = array<number>\nextern xs : Xs\nlocal v : string = xs[1]",
            "不能把",
        );
    }

    // ---- 无标注声明：`local a` 与空表 ----
    //
    // 两条都是「推不出类型就得标注」：`local a` 根本没有类型来源，
    // `{}` 有个空 record 类型但那不是作者想要的形状。空表得按**语法形状**
    // 认，不能看类型 —— 下面 `local b = a` 那条就是钉这个

    /// `local a` 无标注无初值：报
    #[test]
    fn bare_local_needs_annotation() {
        only("local a", "必须带类型标注");
    }

    /// 有标注就干净（number 容不下 nil，inited 从 false 起步，但声明本身不报）
    #[test]
    fn annotated_bare_local_is_clean() {
        clean("local a : number\na = 1");
    }

    /// `local a = {}`：空表推不出形状
    #[test]
    fn empty_table_needs_annotation() {
        only("local a = {}", "空表");
    }

    /// 顶层全局的声明点同一条规矩
    #[test]
    fn empty_table_global_needs_annotation() {
        only("a = {}", "空表");
    }

    /// 标注了容得下空表的类型就干净
    #[test]
    fn annotated_empty_table_is_clean() {
        clean("local a : array<number> = {}");
    }

    /// 非空字面量自己能定形，不在此列
    #[test]
    fn non_empty_table_needs_no_annotation() {
        clean("local a = { n = 1 }");
    }

    /// 拿一个已经定形的空 record 变量再赋给别人不报：这条钉住「按语法形状认」，
    /// 改成看类型就会在这里误报
    #[test]
    fn copying_an_empty_record_is_not_an_empty_literal() {
        clean("local a : {} = {}\nlocal b = a");
    }

    // ---- return：逐位核对、无标注推断、递归须标注 ----
    //
    // 三档分得很死：标了 `->` 就每条 return 逐位核对；没标就反过来、
    // 拿体里所有 return 推一个出来当签名；而推断遇上递归就死循环了，
    // 所以参与递归的函数得自己把返回类型写出来

    /// 标了返回类型，return 的值得塞得进去
    #[test]
    fn return_value_must_fit_the_rettype() {
        clean("local function f() -> number return 1 end");
        only(
            "local function f() -> number return \"x\" end",
            "第 1 个返回值",
        );
    }

    /// 个数也核：多返回值少给一个就报
    #[test]
    fn return_count_must_match() {
        clean("local function f() -> (number, string) return 1, \"x\" end");
        only(
            "local function f() -> (number, string) return 1 end",
            "返回值个数不对",
        );
    }

    /// `-> ()` 是「不返回值」，给了就是错
    #[test]
    fn void_rettype_takes_no_value() {
        clean("local function f() -> () return end");
        only("local function f() -> () return 1 end", "返回值个数不对");
    }

    /// 末尾调用展开成多个返回值，`return g()` 一口气转交两个
    #[test]
    fn return_forwards_a_multi_value_call() {
        clean(concat!(
            "local function g() -> (number, string) return 1, \"x\" end\n",
            "local function f() -> (number, string) return g() end",
        ));
    }

    /// 不标 `->` 就拿体里的 return 推，推出来的类型在调用点上看得见
    #[test]
    fn unannotated_return_is_inferred() {
        clean("local function g() return 1 end\nlocal a : number = g()");
        only(
            "local function g() return 1 end\nlocal a : string = g()",
            "不能把 number 赋给",
        );
    }

    /// 两条 return 逐位并成联类型：number|nil 塞不进 number
    #[test]
    fn multiple_returns_union_position_wise() {
        clean(concat!(
            "local function g(c : boolean) if c then return 1 end return nil end\n",
            "local a : number|nil = g(true)",
        ));
        only(
            concat!(
                "local function g(c : boolean) if c then return 1 end return nil end\n",
                "local a : number = g(true)",
            ),
            "不能把",
        );
    }

    /// 一条 return 也没有：推出空列表，也就是 void
    #[test]
    fn no_return_infers_void() {
        clean("local function g() local n = 1 end\ng()");
    }

    /// 方法也走推断。methodsig 比体先求值，所以这条同时钉住
    /// 「推断得拖到体跑完再装回签名」
    #[test]
    fn method_return_is_inferred_too() {
        clean(concat!(
            "class A { m(self) return 1 end }\n",
            "local function run(a : A) local n : number = a:m() end",
        ));
        only(
            concat!(
                "class A { m(self) return 1 end }\n",
                "local function run(a : A) local n : string = a:m() end",
            ),
            "不能把 number 赋给",
        );
    }

    /// 参与递归而不标返回类型：推导依赖自己，报
    #[test]
    fn recursive_func_needs_ret_annot() {
        only(
            "local function f(n : number) return f(n) end",
            "递归函数 f 必须标注返回类型",
        );
        clean(
            "local function f(n : number) -> number if n > 0 then return f(n - 1) end return 0 end",
        );
    }

    /// 一个体里好几处自引用，只报一次
    #[test]
    fn recursion_diagnostic_fires_once() {
        only(
            "local function f(n : number) local a = f(n) return f(n) end",
            "递归函数 f",
        );
    }

    /// 顶层 `function f` 同一条规矩，而且抬升之后递归调用的实参照常检查
    #[test]
    fn hoisted_recursive_call_checks_args() {
        clean("function f(n : number) -> number if n > 0 then return f(n - 1) end return 0 end");
        only(
            "function f(n : number) -> number if n > 0 then return f(\"x\") end return 0 end",
            "第 1 个实参",
        );
    }

    // ---- prelude 标准库全局 ----
    //
    // 形态上等价于一串 `extern`，于是标准库的名字不必在每个文件顶上重写。
    // 没这一份的话「先声明后使用」会把每个正常文件都刷成一屏未声明

    /// 基础库函数直接能用，不必自己 extern
    #[test]
    fn prelude_declares_base_globals() {
        clean("print(\"hi\")");
        clean("local s : string = tostring(1)");
        clean("local t : string = type(nil)");
        clean("local raw = require(\"cjson\")");
    }

    /// 库表是 record，字段读得出真类型：`string.len` 返 number
    #[test]
    fn prelude_lib_fields_carry_types() {
        clean("local n : number = string.len(\"abc\")");
        only(
            "local n : string = string.len(\"abc\")",
            "不能把 number 赋给",
        );
        // 库表上没有的字段照样报 —— 形状一次定死对库表也算
        only("local x = string.nosuch", "没有字段 nosuch");
    }

    /// 库表叫 `table`、内建类型也叫 `table`：一个在值位一个在类型位，不碰
    #[test]
    fn table_lib_and_table_type_coexist() {
        clean("local t : table<string, number> = {}\ntable.insert(t, 1)");
    }

    /// `error` / `os.exit` 声明成 `-> never`，于是 guard 写法照常：
    /// 落空那一支走不出来，不参与交集
    #[test]
    fn prelude_noreturn_powers_the_guard() {
        clean("local n : number\nif true then n = 1 else error(\"bad\") end\nprint(n)");
        clean("local n : number\nif true then n = 1 else os.exit() end\nprint(n)");
        // assert 不能这么声明：条件为真时它正常返回，堆不成断流
        only(
            "local n : number\nif true then n = 1 else assert(false) end\nprint(n)",
            "可能尚未赋值",
        );
    }

    /// `os.getenv` 返 `string|nil` —— README 的 nil 收窄例子拿它开头
    #[test]
    fn prelude_getenv_is_optional_string() {
        clean("local s : string|nil = os.getenv(\"HOME\")");
        only("local s : string = os.getenv(\"HOME\")", "不能把");
    }

    /// 用户再 extern 一道标准库的名字：名字已经占住了，报重复声明
    #[test]
    fn prelude_names_are_taken() {
        only("extern print : any", "print 已经声明过了");
    }

    /// `_G` 是 `table<string, any>`：README 里 `_G.logger as Logger` 靠它成立
    #[test]
    fn prelude_declares_the_global_table() {
        clean("class Logger { on : boolean }\nlocal l = _G.logger as Logger");
    }

    // ---- nil 收窄 ----
    //
    // 收窄就放在分支块那一格作用域里，出块一弹就没了 ——
    // 不必另建一套回滚。只认裸名字的测试，字段路径不收

    /// README 那一句：`if s ~= nil then print(#s) end` 里 s 是 string
    #[test]
    fn not_nil_guard_narrows_in_then() {
        clean(concat!(
            "local s : string|nil = os.getenv(\"HOME\")\n",
            "if s ~= nil then local t : string = s end",
        ));
        // 没那一道 guard 就收不了
        only(
            "local s : string|nil = os.getenv(\"HOME\")\nlocal t : string = s",
            "不能把",
        );
    }

    /// 两边写反也算，裸名字当条件也算
    #[test]
    fn narrowing_accepts_the_usual_spellings() {
        clean(concat!(
            "local s : string|nil = os.getenv(\"HOME\")\n",
            "if nil ~= s then local t : string = s end",
        ));
        clean(concat!(
            "local s : string|nil = os.getenv(\"HOME\")\n",
            "if s then local t : string = s end",
        ));
        clean(concat!(
            "local s : string|nil = os.getenv(\"HOME\")\n",
            "while s ~= nil do local t : string = s end",
        ));
    }

    /// `if s == nil then … else` 的 else 支里收；then 支里不收
    #[test]
    fn is_nil_guard_narrows_in_else() {
        clean(concat!(
            "local s : string|nil = os.getenv(\"HOME\")\n",
            "if s == nil then local a = 1 else local t : string = s end",
        ));
        only(
            concat!(
                "local s : string|nil = os.getenv(\"HOME\")\n",
                "if s == nil then local t : string = s end",
            ),
            "不能把",
        );
    }

    /// 裸 `if s then` 只正面确切：落空那一支里它可能是 false 而不是 nil
    #[test]
    fn truthy_guard_does_not_narrow_the_else() {
        only(
            concat!(
                "local s : boolean|nil = nil\n",
                "if s then local a = 1 else local t : boolean = s end",
            ),
            "不能把",
        );
    }

    /// 出块就恢复：收窄不泄到分支外面
    #[test]
    fn narrowing_ends_with_the_block() {
        only(
            concat!(
                "local s : string|nil = os.getenv(\"HOME\")\n",
                "if s ~= nil then local a = 1 end\n",
                "local t : string = s",
            ),
            "不能把",
        );
    }

    /// 收窄之后又写回 nil：那条收窄得作废，不然就不健全了
    #[test]
    fn assignment_invalidates_the_narrowing() {
        only(
            concat!(
                "local s : string|nil = os.getenv(\"HOME\")\n",
                "if s ~= nil then s = nil local t : string = s end",
            ),
            "不能把",
        );
    }

    /// 同层再 `local s` 不算重复声明 —— 收窄不是一次声明，它单独一张表
    #[test]
    fn narrowing_is_not_a_declaration() {
        clean(concat!(
            "local s : string|nil = os.getenv(\"HOME\")\n",
            "if s ~= nil then local s : number = 1 end",
        ));
    }

    /// 别名也收：`typedef Opt = string|nil` 先展开再去 nil
    #[test]
    fn narrowing_expands_aliases() {
        clean(concat!(
            "typedef Opt = string|nil\n",
            "local s : Opt = nil\n",
            "if s ~= nil then local t : string = s end",
        ));
    }

    // ---- P4 泛型：class / typedef 形参声明与实例化 ----
    //
    // hoist_decl_generics 在 P0a 把 `<T>` 铸成 GenericId 押进 decl.generics，
    // scope_decl_generics 在 eager/enter 把名字挂进作用域，于是体里的 `T` 解
    // 成 Generic；实例化（`Box<number>`）只记 Ref{decl,args}，查字段时才 subst

    /// 泛型 class 的字段引用形参 `T`：形参进了作用域，不再报「未声明的类型 T」
    #[test]
    fn generic_class_body_param_is_clean() {
        clean("class Box<T> { value : T }");
    }

    /// 实例化时实参个数对不上就报：Box 只收一个形参，给两个要报 arity
    #[test]
    fn generic_class_arity_mismatch_reports() {
        only(
            "class Box<T> { value : T }\nextern b : Box<number, string>",
            "需要",
        );
    }

    /// 端到端代入：Box<number> 的 value 字段把 T 换成 number，接给标注 number 的
    /// local 刚好塔得进、干净。这条同时钓住 lookup_field 的 subst 真的走了
    #[test]
    fn generic_class_field_substitutes() {
        clean("class Box<T> { value : T }\nextern b : Box<number>\nlocal n : number = b.value");
    }

    /// 反面：Box<number> 的 value 代出 number，接给标注 string 的 local 塔不进就报
    #[test]
    fn generic_class_field_subst_mismatch_reports() {
        only(
            "class Box<T> { value : T }\nextern b : Box<number>\nlocal s : string = b.value",
            "不能把",
        );
    }

    /// 泛型 typedef 的目标里引用形参也解析得到：`T` 在作用域里，不报未声明
    #[test]
    fn generic_typedef_param_in_body_is_clean() {
        clean("typedef Box<T> = { value : T }");
    }

    // ---- P4 泛型：函数层形参 ----
    //
    // funcbody 的 `<T>` 和 class/typedef 不同：函数不是名义声明、没有 DeclId，
    // 所以 scope_func_generics 在进 funcbody 时现铸现挂。形参标注 / 返回类型 /
    // 体里的 `T` 解成 Generic；声明序又由 generic_list_of 收进 `Types::Func` 的
    // generics 那格，于是 turbofish 在使用点问得出「第 i 个实参代给谁」

    /// 泛型函数的形参与返回位都引用 `T`：形参进了 funcbody 作用域，不报未声明
    #[test]
    fn generic_function_param_and_return_is_clean() {
        clean("local function id<T>(x : T) -> T return x end");
    }

    /// 体里的 local 也能拿 `T` 标注：泛型形参的可见范围盖住整个体，
    /// 而 Generic 对 assignable 两头都放过，`y : T = x` 塔得进
    #[test]
    fn generic_function_body_local_uses_param_is_clean() {
        clean("function id<T>(x : T) -> T local y : T = x return y end");
    }

    /// 多个形参：A / B 都挂得上，各自解成不同的 Generic
    #[test]
    fn generic_function_multi_params_is_clean() {
        clean("local function pair<A, B>(a : A, b : B) -> A return a end");
    }

    /// 函数字面量（FuncExpr）也走同一条 funcbody，泛型形参照样挂得上
    #[test]
    fn generic_function_expr_is_clean() {
        clean("local f = function<T>(x : T) -> T return x end");
    }

    /// 泛型形参不泄出 funcbody：出体那格作用域一弹，`T` 就不在了，
    /// 顶层再引用就报未声明 —— 钓住 scope_func_generics 挂对了格、leave 也弹对了
    #[test]
    fn generic_function_param_does_not_leak() {
        only("function id<T>(x : T) end\nextern y : T", "未声明的类型 T");
    }

    // ---- 形参上界 `<T : number>` ----
    //
    // resolve_type_param_bound 把上界填进 GenericInfo.constraint，
    // check_generic_bounds 在实例化处（instantiate_type_name / turbofish）逐位校。
    // 没写上界就是「任意类型」，上面那批用例全部依旧

    /// 实参满足上界：`Bounded<number>` 的 number 塞得进 `T : number`，干净
    #[test]
    fn generic_bound_satisfied_is_clean() {
        clean("class Bounded<T : number> { v : T }\nextern b : Bounded<number>");
    }

    /// 反面：string 塞不进 `T : number`，实例化处报越界
    #[test]
    fn generic_bound_violated_reports() {
        only(
            "class Bounded<T : number> { v : T }\nextern b : Bounded<string>",
            "越过了形参",
        );
    }

    /// typedef 上的上界走同一条：形参身份都在 decl.generics 里，不分 class / typedef
    #[test]
    fn generic_bound_on_typedef_violated_reports() {
        only(
            "typedef Bounded<T : number> = { v : T }\nextern b : Bounded<boolean>",
            "越过了形参",
        );
    }

    /// 使用写在声明**之前**照样校得动：顶层 class / typedef 的 generics 在
    /// hoist_class_bodies 的 eager 趟已经求过一遍，主遍历开始前上界已落表。
    /// 这条钓的就是那一趟 —— 若上界只在主遍历里填，这里会静静漏掉
    #[test]
    fn generic_bound_checked_before_declaration() {
        only(
            "extern b : Bounded<string>\nclass Bounded<T : number> { v : T }",
            "越过了形参",
        );
    }

    /// 约束位是单个 type，union 算一个：`T : number|string` 两边都收
    #[test]
    fn generic_bound_union_accepts_either() {
        clean("class N<T : number|string> { v : T }\nextern a : N<number>\nextern b : N<string>");
    }

    /// 实参本身是个未解的形参（`class Wrap<T> : Bounded<T>` 那种透传）时不该报：
    /// assignable 对 Generic 两头都放过，上界得等真正代上类型的那一刻才校
    #[test]
    fn generic_bound_passthrough_param_is_clean() {
        clean("class Bounded<T : number> { v : T }\nclass Wrap<U> { inner : Bounded<U> }");
    }

    // ---- turbofish `f::<number>(x)` ----
    //
    // 形参的声明序记在 `Types::Func` 的 generics 那格，resolve_turbo_fish 从
    // 被调用的那个**函数值**身上问出它，代入后给外层 @Call 去 ret_of

    /// 端到端：`id::<number>` 把 T 代成 number，返回位也跟着成 number，
    /// 接给标注 number 的 local 刚好塞得进
    #[test]
    fn turbo_fish_substitutes_return_type() {
        clean("local function id<T>(x : T) -> T return x end\nlocal n : number = id::<number>(1)");
    }

    /// 反面：代完是 number，接给标注 string 的 local 塞不进就报 ——
    /// 钓住代入真的落在了返回位，而不是回了个 UNKNOWN 水过去
    #[test]
    fn turbo_fish_substitution_mismatch_reports() {
        only(
            "local function id<T>(x : T) -> T return x end\nlocal s : string = id::<number>(1)",
            "不能把",
        );
    }

    /// 多形参按位代：`pair::<number, string>` 的 A 是 number、B 是 string，
    /// 返回位写的是 B，所以接 string 干净 —— 钓住 generics 存的是声明序、
    /// 不是它们在形参表里首次出现的序
    #[test]
    fn turbo_fish_multi_params_map_by_position() {
        clean(
            "local function pair<A, B>(b : B, a : A) -> B return b end\n\
             local s : string = pair::<number, string>(\"x\", 1)",
        );
    }

    /// 实参个数对不上就报：id 只收一个形参
    #[test]
    fn turbo_fish_arity_mismatch_reports() {
        only(
            "local function id<T>(x : T) -> T return x end\nlocal n = id::<number, string>(1)",
            "需要 1 个类型实参",
        );
    }

    /// 非泛型函数给类型实参：generics 那格是空的，报「没有泛型形参」
    #[test]
    fn turbo_fish_on_non_generic_function_reports() {
        only(
            "local function f(x : number) end\nlocal y = f::<number>(1)",
            "没有泛型形参",
        );
    }

    /// 根本不是函数的东西给类型实参：报「不是函数」
    #[test]
    fn turbo_fish_on_non_function_reports() {
        only("extern n : number\nlocal y = n::<number>(1)", "不是函数");
    }

    /// 上界在 turbofish 处也校：`<T : number>` 给个 string 就报越界，
    /// 和 `Bounded<string>` 走的是同一个 check_generic_bounds。
    /// 实参顺着换成 string：不然代完之后第一个实参又会报一道可赋值性
    #[test]
    fn turbo_fish_bound_violated_reports() {
        only(
            "local function inc<T : number>(x : T) -> T return x end\nlocal y = inc::<string>(\"s\")",
            "越过了形参",
        );
    }

    /// 代完之后 generics 那格被 subst 摘空，所以再给一次就不是泛型函数了。
    /// 这条钓住 subst 真的把形参从表上摘了，而不是原封不动地传下去
    #[test]
    fn turbo_fish_twice_reports() {
        only(
            "local function id<T>(x : T) -> T return x end\nlocal y = id::<number>::<string>(1)",
            "没有泛型形参",
        );
    }

    /// 函数字面量也行：走的是同一条 funcbody，generics 同样收进了类型
    #[test]
    fn turbo_fish_on_function_expr_is_clean() {
        clean(
            "local id = function<T>(x : T) -> T return x end\n\
             local n : number = id::<number>(1)",
        );
    }

    // ---- 调用点泛型实参推断 `map(xs, string.len)` ----
    //
    // 不写 `::<>` 也从实参形状反解形参：形参表和实参表逐位比，比到 `T` 就记
    // 一格，最后一次性代入。推得出几个代几个，剩下的仍是形参

    /// 端到端：`id(1)` 反解出 T=number，返回位也成 number，接给 number 干净
    #[test]
    fn infer_substitutes_return_from_arg() {
        clean("local function id<T>(x : T) -> T return x end\nlocal n : number = id(1)");
    }

    /// 反面：反解出 T=number，接给 string 塞不进就报 —— 钓住推断真的落在返回位
    #[test]
    fn infer_return_mismatch_reports() {
        only(
            "local function id<T>(x : T) -> T return x end\nlocal s : string = id(1)",
            "不能把 number 赋给",
        );
    }

    /// README 的例子：`map(xs, string.len)` 从 `array<string>` 反解 T=string、
    /// 从 `string.len` 的返回位反解 U=number，回来的 `array<U>` 成 `array<number>`
    #[test]
    fn infer_map_solves_both_params() {
        clean(concat!(
            "local function map<T, U>(xs : array<T>, f : function(T) -> U) -> array<U>|nil return nil end\n",
            "local function run(xs : array<string>) local ys : array<number>|nil = map(xs, string.len) end",
        ));
        only(
            concat!(
                "local function map<T, U>(xs : array<T>, f : function(T) -> U) -> array<U>|nil return nil end\n",
                "local function run(xs : array<string>) local ys : array<string>|nil = map(xs, string.len) end",
            ),
            "不能把",
        );
    }

    /// 容器实参钻进去解：`array<T>` 对上 `array<string>` 解出 T=string，
    /// 返回位 `T|nil` 跟着成 `string|nil`
    #[test]
    fn infer_reaches_into_containers() {
        clean(concat!(
            "local function first<T>(xs : array<T>) -> T|nil return nil end\n",
            "local function run(xs : array<string>) local s : string|nil = first(xs) end",
        ));
    }

    /// 别名先展开再比：`typedef Names = array<string>` 传给 `array<T>` 照样解出 T=string
    #[test]
    fn infer_unwraps_alias_before_matching() {
        clean(concat!(
            "local function first<T>(xs : array<T>) -> T|nil return nil end\n",
            "typedef Names = array<string>\n",
            "local function run(xs : Names) local s : string|nil = first(xs) end",
        ));
    }

    /// 上界照样管推出来的实参：`<T : number>` 从 string 实参反解出 T=string 就报越界。
    /// 钓住 check_generic_bounds 在推断点也接上了，不只 turbofish 那条
    #[test]
    fn infer_still_enforces_bounds() {
        only(
            "local function inc<T : number>(x : T) -> T return x end\nlocal y = inc(\"s\")",
            "越过了形参",
        );
    }

    /// 一个形参只认第一次命中：`f(a:T, b:T)` 先从 1 解出 T=number，第 2 个实参
    /// 给 string 就按 T=number 去核 —— 报的是「第 2 个实参」而不是一句「推断失败」
    #[test]
    fn infer_first_binding_wins_then_checks_rest() {
        only(
            "local function two<T>(a : T, b : T) -> T return a end\nlocal y = two(1, \"s\")",
            "第 2 个实参",
        );
    }

    /// 推不出来（没有实参能定 T）时不硬猜：T 留作形参、对 assignable 放行，
    /// 所以不误报；要真把它定下来就写 turbofish，返回位跟着变具体
    #[test]
    fn infer_leaves_unsolved_generic_permissive() {
        clean("local function make<T>() -> T|nil return nil end\nlocal n = make()");
        clean(
            "local function make<T>() -> T|nil return nil end\nlocal n : number|nil = make::<number>()",
        );
    }

    // ---- 跨文件 ----

    /// 手工串两次 `run`：同一个 TypeLinter 依次跑两个文件，每个文件
    /// 自己一份诊断。两棵树各自随 parser 当场析构 —— linter 不存树，
    /// 存了这里当场编译不过
    fn lint_all(srcs: &[&str]) -> Vec<Vec<String>> {
        let mut driver = LintDriver::new().init(TypeLinter::new());
        let mut out = Vec::new();
        for (i, &src) in srcs.iter().enumerate() {
            let mut parser = Parser::new(src);
            let (ast, _) = parser.parse();
            // 模块名只要彼此不撞就行，这组测的是累积 / 不泄露，不涉跨模块引用
            let module = format!("m{i}");
            out.push(
                driver
                    .run(ast, &module)
                    .into_iter()
                    .map(|e| e.msg)
                    .collect(),
            );
        }
        out
    }

    /// 同 lint_all，但每个文件自带模块名—— 跨模块 import 要在 STRING 里
    /// 按名引用，模块名得可控
    fn lint_modules(mods: &[(&str, &str)]) -> Vec<Vec<String>> {
        let mut driver = LintDriver::new().init(TypeLinter::new());
        let mut out = Vec::new();
        for &(module, src) in mods {
            let mut parser = Parser::new(src);
            let (ast, _) = parser.parse();
            out.push(driver.run(ast, module).into_iter().map(|e| e.msg).collect());
        }
        out
    }

    /// 全局跟着 Session 跑过文件边界：A 里声明的 `G`，B 里读得到。
    /// 反面：单跑 B 必报未声明，否则这条测的就不是累积而是「全局不查」
    #[test]
    fn globals_accumulate_across_files() {
        let got = lint_all(&["G = 1", "local a = G"]);
        assert!(got[0].is_empty(), "第一个文件不该有诊断，实得 {:?}", got[0]);
        assert!(got[1].is_empty(), "第二个文件该看得见 G，实得 {:?}", got[1]);
        only("local a = G", "未声明的变量 G");
    }

    /// 局部反过来：per-file 的 scope_stack 得在 `prepare` 里真清掉
    #[test]
    fn locals_do_not_leak_across_files() {
        let got = lint_all(&["local x = 1", "local a = x"]);
        assert!(got[0].is_empty(), "实得 {:?}", got[0]);
        assert_eq!(got[1].len(), 1, "实得 {:?}", got[1]);
        assert!(got[1][0].contains("未声明的变量 x"), "实得 {:?}", got[1]);
    }

    /// 同一份源码跑两次，两次结论得一模一样。这一条同时盯三件事：
    /// `node_values` 没清会把 `register_value` 的重入断言敲响、日志没按文件
    /// 取走会让第二轮变成两条、`finish` 的配平断言在第二轮也得成立
    #[test]
    fn rerunning_the_same_source_is_idempotent() {
        let got = lint_all(&[
            "local n : number\nlocal m = n",
            "local n : number\nlocal m = n",
        ]);
        assert_eq!(got[0].len(), 1, "实得 {:?}", got[0]);
        assert_eq!(got[0], got[1], "两轮该一模一样");
    }

    /// 全局的声明点是唯一权威且跨文件：A 里声明过的名字，B 里再赋值
    /// 不是新声明，所以写在非顶层也不该报「声明点只能在文件顶层」
    #[test]
    fn assigning_a_known_global_from_a_block_is_fine() {
        let got = lint_all(&["G = 1", "do G = 2 end"]);
        assert!(got[1].is_empty(), "实得 {:?}", got[1]);
        only("do G = 2 end", "声明点只能在文件顶层");
    }

    // ---- 跨模块 import / pub ----

    /// 一趟完整往返：a 里 pub 一个 class，b 里 import 回来当类型名用，
    /// 两个文件都干净。export 进了 Session、b 里绑对了名字才能都不报
    #[test]
    fn import_of_pub_type_roundtrips_clean() {
        let got = lint_modules(&[
            ("a", "pub class Point { x : number }"),
            ("b", "import { Point } in \"a\"\nextern p : Point"),
        ]);
        assert!(got[0].is_empty(), "a 不该有诊断，实得 {:?}", got[0]);
        assert!(got[1].is_empty(), "b 该看得见 Point，实得 {:?}", got[1]);
    }

    /// `as` 改名：本地名绑对。还顺带钓一下原名不泄露——
    /// import 了 `Point as P` 之后，再写 Point 应当是未声明
    #[test]
    fn import_alias_binds_local_name() {
        let got = lint_modules(&[
            ("a", "pub class Point { x : number }"),
            ("b", "import { Point as P } in \"a\"\nextern p : P"),
        ]);
        assert!(got[0].is_empty(), "实得 {:?}", got[0]);
        assert!(got[1].is_empty(), "别名该绑对，实得 {:?}", got[1]);

        let leak = lint_modules(&[
            ("a", "pub class Point { x : number }"),
            ("b", "import { Point as P } in \"a\"\nextern p : Point"),
        ]);
        assert!(
            leak[1].iter().any(|m| m.contains("未声明的类型 Point")),
            "原名不该泄露，实得 {:?}",
            leak[1]
        );
    }

    /// import 一个没 pub 的类型：模块里真有这名字，只是没导出
    #[test]
    fn import_of_non_pub_reports() {
        let got = lint_modules(&[
            ("a", "class Secret { x : number }"),
            ("b", "import { Secret } in \"a\""),
        ]);
        assert!(got[0].is_empty(), "a 不该有诊断，实得 {:?}", got[0]);
        assert_eq!(got[1].len(), 1, "实得 {:?}", got[1]);
        assert!(
            got[1][0].contains("类型 Secret 没有 pub"),
            "实得 {:?}",
            got[1]
        );
    }

    /// 模块里压根没这个类型：跟「有但没 pub」报不同的错
    #[test]
    fn import_of_unknown_name_reports() {
        let got = lint_modules(&[
            ("a", "pub class Point { x : number }"),
            ("b", "import { Ghost } in \"a\""),
        ]);
        assert_eq!(got[1].len(), 1, "实得 {:?}", got[1]);
        assert!(
            got[1][0].contains("模块 a 没有导出类型 Ghost"),
            "实得 {:?}",
            got[1]
        );
    }

    /// 压根不存在的模块：module_has_type 落空，归到「模块没导出」一支
    #[test]
    fn import_from_unknown_module_reports() {
        only(
            "import { Point } in \"nope\"",
            "模块 nope 没有导出类型 Point",
        );
    }

    /// import 回来的名字撞上本文件已有的同名类型
    #[test]
    fn import_name_collision_reports() {
        let got = lint_modules(&[
            ("a", "pub class Point { x : number }"),
            ("b", "class Point { y : number }\nimport { Point } in \"a\""),
        ]);
        assert_eq!(got[1].len(), 1, "实得 {:?}", got[1]);
        assert!(
            got[1][0].contains("Point 已经声明过了"),
            "实得 {:?}",
            got[1]
        );
    }

    /// 同一模块的两个文件都 pub 了同名类型：第二次导出撞名。
    /// 同文件内的重名早被 check_class_decl 挡了，这条钓的是跨文件那一层
    #[test]
    fn duplicate_pub_export_in_same_module_reports() {
        let got = lint_modules(&[
            ("shared", "pub class Point { x : number }"),
            ("shared", "pub class Point { y : number }"),
        ]);
        assert!(got[0].is_empty(), "头一份不该有诊断，实得 {:?}", got[0]);
        assert_eq!(got[1].len(), 1, "实得 {:?}", got[1]);
        assert!(
            got[1][0].contains("类型 Point 已经被导出过了"),
            "实得 {:?}",
            got[1]
        );
    }

    /// pub 只能贴顶层：嵌在 do 块里的得拦下来（跟 extern 一个道理）
    #[test]
    fn pub_on_non_toplevel_reports() {
        only(
            "do pub class Point { x : number } end",
            "pub 只能修饰顶层的 class / typedef",
        );
    }

    // ---- 整棵样例 ----

    /// README sample 的可运行版：`examples/sample.tlua` 是 README 语言参考里
    /// 那份示例的落地文件，整篇应当零诊断 —— 端到端把「设计」和「实现」钉在一起。
    /// `examples/pkg/account.tlua` 是它 import 的模块，先按 `pkg.account` 灌进
    /// 同一个 driver，sample 里的跨文件引用才解析得到
    #[test]
    fn readme_sample_is_clean_end_to_end() {
        let account = std::fs::read_to_string("examples/pkg/account.tlua").unwrap();
        let sample = std::fs::read_to_string("examples/sample.tlua").unwrap();
        let got = lint_modules(&[("pkg.account", &account), ("main", &sample)]);
        assert!(
            got[0].is_empty(),
            "account.tlua 该零诊断，实得 {:?}",
            got[0]
        );
        assert!(got[1].is_empty(), "sample.tlua 该零诊断，实得 {:?}", got[1]);
    }

    /// examples/ 里的东西至少不能把驱动点跑崩。demo.tlua 是负面展示（下半部分
    /// 每条都该报），只断言「不崩」不断言数量 —— 重点在 `finish` 里那三条配平
    /// 断言，它们只在 debug 下才响，而测试就是 debug
    #[test]
    fn examples_survive_the_driver() {
        let account = std::fs::read_to_string("examples/pkg/account.tlua").unwrap();
        let demo = std::fs::read_to_string("examples/demo.tlua").unwrap();
        let got = lint_modules(&[("pkg.account", &account), ("demo", &demo)]);
        println!("demo.tlua -> {:?}", got[1]);
    }
}
