//! 截图后自动展开的预览：8 秒后收起；鼠标进入后保持展开，离开后按离开缓冲收起。
//! 与验证码预览相同，是覆盖层，不改变用户原先停留的页面。
use super::*;

const DURATION: f64 = 8.0;
/// 保存失败时多留一会儿，给用户时间选择重试或另选目录。
const FAILURE_DURATION: f64 = 20.0;

impl App {
    /// `seq`：这次截图的序号，用于对应复制结果；保存失败和重试保存时为 `None`。
    pub(crate) fn start_screenshot_preview(&mut self, seq: Option<u64>) {
        // 正在编辑草稿、拖放文件或任务对话框打开：不弹出，只提示，避免打断或丢失输入。
        if self.is_editing() || self.drag_hot || self.task_dialog.is_some() {
            self.say(if self.shots.failure.is_some() { "截图未能保存：完成当前操作后，再按截图快捷键处理" } else { "截图已保存" });
            return;
        }
        let duration = if self.shots.failure.is_some() { FAILURE_DURATION } else { DURATION };
        let copy = self.take_early_copy(seq);
        self.shots.preview = Some(crate::screenshot_ui::Preview { until: Some(self.t + duration), copy, left_at: None, seq });
        self.open = true;
        self.hover_lock = false;
        self.hover_since = None;
        self.leave_since = None;
        self.forced_open_until = 0.0;
        self.w.tune(OPEN_W.0, OPEN_W.1); self.h.tune(OPEN_H.0, OPEN_H.1);
        self.w.target = PW; self.h.target = crate::screenshot_ui::HEIGHT;
    }

    /// 每帧调用；`inside` 为光标是否在岛上。
    pub(super) fn tick_screenshot_preview(&mut self, inside: bool) {
        let t = self.t;
        let Some(preview) = self.shots.preview.as_mut() else { return };
        if inside { preview.until = None; preview.left_at = None; return; }
        let expired = match preview.until {
            Some(until) => t >= until,
            // 进入过岛：离开后沿用现有的离开缓冲。
            None => t - *preview.left_at.get_or_insert(t) >= LEAVE_DELAY,
        };
        if expired { self.end_screenshot_preview(true); }
    }

    pub(crate) fn end_screenshot_preview(&mut self, collapse: bool) {
        if self.shots.preview.take().is_none() { return; }
        self.forced_open_until = 0.0;
        // 预览期间到达的验证码还在有效期内：接着显示验证码预览，不收起。
        if self.mail.preview.is_some() { self.w.target = PW; self.h.target = self.expanded_height(); return; }
        if collapse {
            self.open = false;
            self.hover_lock = true;
            self.hover_since = None;
            self.leave_since = None;
            self.w.tune(CLOSE.0, CLOSE.1); self.h.tune(CLOSE.0, CLOSE.1);
            let (width, height) = self.base_size(); self.w.target = width; self.h.target = height;
        }
    }
}
