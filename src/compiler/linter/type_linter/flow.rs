//! Split out of type_linter.rs. Methods stay inherent on TypeLinter;
//! `use super::*;` pulls in the struct, helper types and imports.
use super::*;

impl TypeLinter {
    // ================== §4 定值分析 ==================

    // ---- 定值分析：分支合并 ----
    //
    // 赋值只会把 `inited` 从 false 推到 true（`n = nil` 对 `number` 本来非法，
    // 含 nil 的类型又一开始就是 true），所以这是个**单调**框架：不建 CFG、
    // 不做定点迭代，一遍前序遍历配上快照 / 回滚 / 取交集就够。
    //
    // 谁调：驱动点按语句种类配对调 `flow_open` / `flow_close`，每个分支块
    // 配对调 `flow_enter_branch` / `flow_leave_branch`：
    //   `if … end`                     -> open, 各分支, close(Skippable)
    //   `if … else … end`              -> open, 各分支, close(Exhaustive)
    //   `while` / `for` / `for in`      -> open, 体, close(Skippable)    可能 0 次
    //   `do … end` / `repeat … until`  -> open, 体, close(Always)       至少 1 次
    //   funcbody                        -> open, flow_enter_body, close(Skippable)
    // 分支块自己那层作用域要在 `flow_enter_branch` **之后**才入栈，
    // 否则它会被当成外层作用域，内层遮蔽同名变量时回滚就打错人

    /// per-file 重置。`prepare` 里**先压文件级作用域、再调这个**；
    /// 栈底那格「文件帧」是所有 `flow_*` 的地基，不压就会 `expect` 响
    pub(super) fn flow_reset(&mut self) {
        self.init_record.clear();
        self.join_stack.clear();
        self.flow_stack.clear();
        // 文件 chunk 在 Lua 里就是个函数，所以它是边界
        self.push_frame(true);
    }

    pub(super) fn push_frame(&mut self, is_fn_body: bool) {
        self.flow_stack.push(FlowFrame {
            record_mark: self.init_record.len(),
            scope_depth: self.scope_stack.len(),
            terminated: false,
            dead_reported: false,
            untrusted: false,
            is_fn_body,
        });
    }

    /// 进一条带分支的语句（`if` / 循环 / `do` / funcbody）
    pub(super) fn flow_open(&mut self) {
        self.join_stack.push(Vec::new());
    }

    /// 进一个分支块
    pub(super) fn flow_enter_branch(&mut self) {
        self.push_frame(false);
    }

    /// 进一个函数体。和普通分支块差在两处：体内的 `return` 不能终结外层
    /// （`local f = function() return end` 之后外层照样往下跑），`goto` 的降级
    /// 也到这里为止。两件事共用 `is_fn_body` 一位
    pub(super) fn flow_enter_body(&mut self) {
        self.push_frame(true);
    }

    /// 离开分支块：把本分支的翻转全回滚，结果攒进当前语句。
    /// 回滚是必需的：分支里赋了值不代表跑到外面也赋了，得等所有
    /// 分支跑完取完交集才能定
    pub(super) fn flow_leave_branch(&mut self) {
        let frame = self.flow_stack.pop().expect("分支块得先 flow_enter_branch");
        let flips: Vec<InitFlip> = self
            .init_record
            .split_off(frame.record_mark)
            .into_iter()
            // 落在已经弹掉的内层作用域里：那个变量出了分支就不存在，
            // 带出去会打到被它遮蔽的外层同名变量
            .filter(|f| f.scope.is_none_or(|d| d < frame.scope_depth))
            .collect();
        for flip in &flips {
            self.set_inited(flip, false);
        }
        self.join_stack
            .last_mut()
            .expect("分支块得在 flow_open 之内")
            .push(BranchExit {
                flips,
                terminated: frame.terminated,
            });
    }

    /// 离开这条语句，按 mode 把各分支的结果合进外层帧
    pub(super) fn flow_close(&mut self, mode: JoinMode) {
        let branches = self
            .join_stack
            .pop()
            .expect("flow_close 得和 flow_open 配对");
        // 落空路径存在 -> 交集必然为空，而且既然能落空就永远终结不了外层
        if mode == JoinMode::Skippable || branches.is_empty() {
            return;
        }
        let joined = {
            let mut live = branches.iter().filter(|b| !b.terminated);
            let Some(first) = live.next() else {
                // 一条分支都走不出来（`if c then return else error("x") end`）：
                // 外层从这条语句起也到不了下一句
                self.flow_terminate();
                return;
            };
            let mut acc = first.flips.clone();
            for b in live {
                acc.retain(|f| b.flips.contains(f));
            }
            acc
        };
        self.flow_apply(joined);
    }

    /// 把合并出来的置位写进外层帧。**必须再记一遍日志**：外层要是
    /// 自己也是个分支（嵌套的 `if`），它退出时还得能把这些一起回滚
    pub(super) fn flow_apply(&mut self, flips: Vec<InitFlip>) {
        for flip in flips {
            self.set_inited(&flip, true);
            self.init_record.push(flip);
        }
    }

    /// 按记录的位置就地写 `inited`。**不按名字重新查表**：当时那次置位
    /// 打的是哪一格，回滚 / 应用就必须打回同一格
    pub(super) fn set_inited(&mut self, flip: &InitFlip, inited: bool) {
        match flip.scope {
            Some(depth) => {
                if let Some(slot) = self
                    .scope_stack
                    .get_mut(depth)
                    .and_then(|s| s.variables.get_mut(&flip.name))
                {
                    slot.inited = inited;
                }
            }
            None => self.session.set_global_inited(&flip.name, inited),
        }
    }

    /// 当前帧走不下去了：`return` / `break` / `goto` / 调了不返回的函数
    pub(super) fn flow_terminate(&mut self) {
        if let Some(frame) = self.flow_stack.last_mut() {
            frame.terminated = true;
        }
    }

    /// 当前帧已经终结 —— 后面的语句是死代码
    pub(super) fn flow_terminated(&self) -> bool {
        self.flow_stack.last().is_some_and(|f| f.terminated)
    }

    /// 帧又活了：碰上 label。`goto skip` 把帧终结掉，可 `::skip::` 恰恰是那一跳
    /// 的落点 —— 不救活就会把落点自己报成死代码。
    ///
    /// 只清 `terminated` 不清 `dead_reported`：既然这一段的可达性已经不可靠，
    /// 就别在同一帧里第二次开口
    pub(super) fn flow_revive(&mut self) {
        if let Some(frame) = self.flow_stack.last_mut() {
            frame.terminated = false;
        }
    }

    /// 碰上 `goto` / label：把当前帧到最近一个函数边界都标成不可信。
    /// 向后跳让「一遍前序合并」失效，而此时宁可漏报不误报。
    ///
    /// 它只能降级 `goto` **之后**看到的语句 —— 前序遍历还没走到后面那个
    /// `goto` 时它无从得知。要全程降级得在进函数体时浅扫一遍找 `goto`；
    /// Lua 本身禁止 goto 跳进 local 的作用域，漏的那一小块很窄
    pub(super) fn flow_untrust(&mut self) {
        for frame in self.flow_stack.iter_mut().rev() {
            frame.untrusted = true;
            if frame.is_fn_body {
                break;
            }
        }
    }

    /// 当前函数里的 inited 结论还信得过。返回 false 时该把「可能尚未赋值」
    /// 一律放过（类型推导不受影响，只关掉这一条诊断）
    pub(super) fn flow_trusted(&self) -> bool {
        for frame in self.flow_stack.iter().rev() {
            if frame.untrusted {
                return false;
            }
            if frame.is_fn_body {
                break;
            }
        }
        true
    }

    /// 最内层的函数体那格帧，返回（它在栈里的下标, 它进来时的作用域深度）。
    /// 下标 0 就是 `flow_reset` 压的文件帧，也就是「立即执行位置」的判据
    pub(super) fn fn_frame(&self) -> (usize, usize) {
        self.flow_stack
            .iter()
            .enumerate()
            .rev()
            .find(|(_, f)| f.is_fn_body)
            .map(|(i, f)| (i, f.scope_depth))
            .expect("文件帧是 flow_reset 压的，一定在")
    }

    /// 这个变量的「可能尚未赋值」在当前位置算不算得准。
    /// `scope` 是它落在哪一层（`lookup_variable_at` 的第一项）。
    ///
    /// 分界线是「声明与读是不是在同一遍前序里排好序的」：
    ///   - 当前函数自己的局部：是，查
    ///   - 外层函数的局部（upvalue）与全局：不是 —— 闭包可能等到赋值之后才被
    ///     调用，所以只有最内层的函数帧就是文件本体（立即执行）时才敢查。
    ///     `a = function() b() end` 这种互递归前向声明靠的就是这一条
    pub(super) fn init_checkable(&self, scope: Option<usize>) -> bool {
        // 段内有 goto：结论本身不可信，不论变量归谁都放过
        if !self.flow_trusted() {
            return false;
        }
        let (frame_index, base) = self.fn_frame();
        match scope {
            Some(depth) => depth >= base,
            None => frame_index == 0,
        }
    }

    /// 这条 @ExprStat 是不是「调了就走不出去」的调用（`error("bad")`）。
    /// 没它的话 guard 写法会误报：
    /// `local n:number; if c then n = 1 else error("bad") end; print(n)`
    ///
    /// 判据在**类型**上：调用求出来的是 `never`，
    /// 也就是它根本不产出值、控制流到这里就断了。
    ///
    /// 不看被调者写成什么样，于是 `os.exit()`（字段读）和 `local e = error`
    /// 之后的 `e("x")` 都自动成立 —— 类型跟着值跑，不像标记位只能贴在裸名字上。
    /// 代价是它只能在 `leave` 里问：调用的类型是后序算出来的，`enter` 时
    /// `node_values` 里还没有它。这不影响正确性 —— 语句执行完才谈得上终结。
    ///
    /// 另外：`assert` 不能声明成 `-> never`。条件为真时它原样返回第一个实参
    pub(super) fn is_noreturn_stat(&self, ast: &Tree<NodeSyntax<'_>>, node_index: usize) -> bool {
        // stat : prefixexp @ExprStat —— 带标签的单孩子不折叠，底下才是那个调用
        let Some(&call) = ast.get_node(node_index).children.first() else {
            return false;
        };
        // 截成单值再比：`-> never` 的 ret 本来就折叠成 never 自己，
        // 走一趟 truncate_ret 只是为了不假设它一定没被包进 Pack
        self.truncate_ret(self.child_type(call)) == TypeId::NEVER
    }
}
