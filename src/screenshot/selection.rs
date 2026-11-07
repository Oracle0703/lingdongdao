//! 截图选区的纯物理像素几何；不读取鼠标、不接触窗口，也不触发保存。

pub(super) type Point = (i32, i32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PixelRect {
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) right: i32,
    pub(super) bottom: i32,
}

impl PixelRect {
    pub(super) fn width(self) -> i32 { self.right.saturating_sub(self.left) }
    pub(super) fn height(self) -> i32 { self.bottom.saturating_sub(self.top) }
    pub(super) fn valid(self) -> bool { self.width() >= 2 && self.height() >= 2 }
    pub(super) fn contains(self, point: Point) -> bool {
        point.0 >= self.left && point.0 < self.right && point.1 >= self.top && point.1 < self.bottom
    }
    pub(super) fn normalized(self) -> Self {
        Self {left:self.left.min(self.right), top:self.top.min(self.bottom),
            right:self.left.max(self.right), bottom:self.top.max(self.bottom)}
    }
    pub(super) fn intersect(self, other: Self) -> Option<Self> {
        let rect=Self {left:self.left.max(other.left), top:self.top.max(other.top),
            right:self.right.min(other.right), bottom:self.bottom.min(other.bottom)};
        (rect.width()>0 && rect.height()>0).then_some(rect)
    }
    pub(super) fn union(self, other: Self) -> Self {
        Self {left:self.left.min(other.left), top:self.top.min(other.top),
            right:self.right.max(other.right), bottom:self.bottom.max(other.bottom)}
    }
    pub(super) fn expand(self, pixels: i32) -> Self {
        Self {left:self.left.saturating_sub(pixels), top:self.top.saturating_sub(pixels),
            right:self.right.saturating_add(pixels), bottom:self.bottom.saturating_add(pixels)}
    }
    pub(super) fn offset(self, x: i32, y: i32) -> Self {
        Self {left:self.left.saturating_add(x), top:self.top.saturating_add(y),
            right:self.right.saturating_add(x), bottom:self.bottom.saturating_add(y)}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Grip { NorthWest, North, NorthEast, East, SouthEast, South, SouthWest, West, Move }

impl Grip {
    fn corner_priority(self) -> u8 {
        match self {Self::NorthWest|Self::NorthEast|Self::SouthEast|Self::SouthWest=>0,_=>1}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hit { Grip(Grip), Inside, Outside, Cancel, Save, Toolbar }

/// 全部值已经由所在屏幕的 DPI 换算为物理像素。
#[derive(Clone, Copy, Debug)]
pub(super) struct SelectionMetrics {
    pub(super) handle_size: i32,
    /// 绘制方块之外再增加的命中容差；不是相对中心的总半径。
    pub(super) hit_padding: i32,
    pub(super) toolbar_gap: i32,
    pub(super) button_width: i32,
    pub(super) button_height: i32,
    pub(super) button_gap: i32,
    pub(super) padding: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ToolbarLayout {
    pub(super) bounds: PixelRect,
    pub(super) cancel: PixelRect,
    pub(super) save: PixelRect,
}

fn coordinate(value: i64) -> i32 { value.clamp(i32::MIN as i64,i32::MAX as i64) as i32 }
fn midpoint(a: i32, b: i32) -> i32 { coordinate(a as i64+(b as i64-a as i64)/2) }

pub(super) fn handles(rect: PixelRect, metrics: SelectionMetrics) -> [(Grip, PixelRect); 8] {
    let rect=rect.normalized();
    let (cx,cy)=(midpoint(rect.left,rect.right),midpoint(rect.top,rect.bottom));
    let points=[(Grip::NorthWest,(rect.left,rect.top)),(Grip::North,(cx,rect.top)),
        (Grip::NorthEast,(rect.right,rect.top)),(Grip::East,(rect.right,cy)),
        (Grip::SouthEast,(rect.right,rect.bottom)),(Grip::South,(cx,rect.bottom)),
        (Grip::SouthWest,(rect.left,rect.bottom)),(Grip::West,(rect.left,cy))];
    let size=metrics.handle_size.max(1) as i64;
    points.map(|(grip,(x,y))| {
        let left=x as i64-size/2;let top=y as i64-size/2;
        (grip,PixelRect {left:coordinate(left),top:coordinate(top),right:coordinate(left+size),bottom:coordinate(top+size)})
    })
}

/// 工具栏首先置于选区下方，再尝试上方，最后放在选区右下角内侧。
/// 极小显示区域仍优先保证整个工具栏及按钮不超出屏幕。
pub(super) fn toolbar_layout(rect: PixelRect, bounds: PixelRect, metrics: SelectionMetrics) -> ToolbarLayout {
    let rect=rect.normalized();let bounds=bounds.normalized();
    let padding=metrics.padding.max(0) as i64;
    let gap=metrics.toolbar_gap.max(0) as i64;
    let button_gap=metrics.button_gap.max(0) as i64;
    let width=(metrics.button_width.max(1) as i64*2+button_gap+padding*2)
        .min(bounds.right as i64-bounds.left as i64);
    let height=(metrics.button_height.max(1) as i64+padding*2)
        .min(bounds.bottom as i64-bounds.top as i64);
    let below=rect.bottom as i64+gap;
    let above=rect.top as i64-gap-height;
    let (x,y)=if below>=bounds.top as i64 && below+height<=bounds.bottom as i64 {
        (rect.right as i64-width,below)
    } else if above>=bounds.top as i64 && above+height<=bounds.bottom as i64 {
        (rect.right as i64-width,above)
    } else {
        (rect.right as i64-gap-width,rect.bottom as i64-gap-height)
    };
    let x=x.clamp(bounds.left as i64,bounds.right as i64-width);
    let y=y.clamp(bounds.top as i64,bounds.bottom as i64-height);
    let panel=PixelRect {left:coordinate(x),top:coordinate(y),right:coordinate(x+width),bottom:coordinate(y+height)};
    // 在很窄的屏幕上先压缩内边距和按钮间隙，尽量给两个按钮各保留一个像素。
    let px=padding.min((width-2).max(0)/2);
    let py=padding.min((height-1).max(0)/2);
    let inner_width=width-px*2;let inner_height=height-py*2;
    let button_gap=button_gap.min((inner_width-2).max(0));
    let cancel_width=(inner_width-button_gap)/2;
    let cancel=PixelRect {left:coordinate(x+px),top:coordinate(y+py),
        right:coordinate(x+px+cancel_width),bottom:coordinate(y+py+inner_height)};
    let save=PixelRect {left:coordinate(x+px+cancel_width+button_gap),top:cancel.top,
        right:coordinate(x+width-px),bottom:cancel.bottom};
    ToolbarLayout {bounds:panel,cancel,save}
}

/// 工具栏优先于控制点；狭窄选区的控制点命中区重叠时选最近中心，平局优先角点。
/// Save 命中不表示可以保存；调用方仍须用 rect.valid() 禁用极小选区。
pub(super) fn hit_test(rect: PixelRect, point: Point, bounds: PixelRect, metrics: SelectionMetrics) -> Hit {
    let toolbar=toolbar_layout(rect,bounds,metrics);
    if toolbar.cancel.contains(point) {return Hit::Cancel;}
    if toolbar.save.contains(point) {return Hit::Save;}
    if toolbar.bounds.contains(point) {return Hit::Toolbar;}
    let grip=handles(rect,metrics).into_iter().filter(|(_,handle)|handle.expand(metrics.hit_padding.max(0)).contains(point))
        .min_by_key(|(grip,handle)| {
            let dx=point.0 as i128-midpoint(handle.left,handle.right) as i128;
            let dy=point.1 as i128-midpoint(handle.top,handle.bottom) as i128;
            (dx*dx+dy*dy,grip.corner_priority())
        }).map(|(grip,_)|grip);
    if let Some(grip)=grip {return Hit::Grip(grip);}
    if rect.normalized().contains(point) {Hit::Inside}else{Hit::Outside}
}

/// 每次使用按下时的原矩形和相对按下点的总位移，避免越过对边后切换锚点。
pub(super) fn apply_drag(rect: PixelRect, grip: Grip, delta: Point, bounds: PixelRect) -> PixelRect {
    let rect=rect.normalized();let bounds=bounds.normalized();
    let (left,top,right,bottom)=(bounds.left as i64,bounds.top as i64,bounds.right as i64,bounds.bottom as i64);
    if grip==Grip::Move {
        let width=(rect.right as i64-rect.left as i64).min(right-left);
        let height=(rect.bottom as i64-rect.top as i64).min(bottom-top);
        let x=(rect.left as i64+delta.0 as i64).clamp(left,right-width);
        let y=(rect.top as i64+delta.1 as i64).clamp(top,bottom-height);
        return PixelRect {left:coordinate(x),top:coordinate(y),right:coordinate(x+width),bottom:coordinate(y+height)};
    }
    let mut x1=(rect.left as i64).clamp(left,right);let mut x2=(rect.right as i64).clamp(left,right);
    let mut y1=(rect.top as i64).clamp(top,bottom);let mut y2=(rect.bottom as i64).clamp(top,bottom);
    if matches!(grip,Grip::NorthWest|Grip::West|Grip::SouthWest) {x1=(rect.left as i64+delta.0 as i64).clamp(left,right);}
    if matches!(grip,Grip::NorthEast|Grip::East|Grip::SouthEast) {x2=(rect.right as i64+delta.0 as i64).clamp(left,right);}
    if matches!(grip,Grip::NorthWest|Grip::North|Grip::NorthEast) {y1=(rect.top as i64+delta.1 as i64).clamp(top,bottom);}
    if matches!(grip,Grip::SouthWest|Grip::South|Grip::SouthEast) {y2=(rect.bottom as i64+delta.1 as i64).clamp(top,bottom);}
    PixelRect {left:coordinate(x1.min(x2)),top:coordinate(y1.min(y2)),right:coordinate(x1.max(x2)),bottom:coordinate(y1.max(y2))}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left:i32,top:i32,right:i32,bottom:i32)->PixelRect {PixelRect {left,top,right,bottom}}
    fn metrics(dpi:i32)->SelectionMetrics {
        let px=|logical:i32|(logical*dpi+48)/96;
        SelectionMetrics {handle_size:px(8),hit_padding:px(6),toolbar_gap:px(6),
            button_width:px(80),button_height:px(32),button_gap:px(8),padding:px(6)}
    }
    fn center(rect:PixelRect)->Point {(midpoint(rect.left,rect.right),midpoint(rect.top,rect.bottom))}
    fn contained(inner:PixelRect,outer:PixelRect)->bool {
        inner.left>=outer.left && inner.top>=outer.top && inner.right<=outer.right && inner.bottom<=outer.bottom
    }

    #[test]
    fn eight_grips_have_shared_paint_and_hit_geometry_at_each_dpi() {
        let selection=rect(200,200,800,600);let screen=rect(0,0,1920,1080);
        let expected=[(Grip::NorthWest,(200,200)),(Grip::North,(500,200)),(Grip::NorthEast,(800,200)),
            (Grip::East,(800,400)),(Grip::SouthEast,(800,600)),(Grip::South,(500,600)),
            (Grip::SouthWest,(200,600)),(Grip::West,(200,400))];
        for dpi in [96,120,144,192] {
            let metrics=metrics(dpi);
            for ((grip,handle),(expected_grip,point)) in handles(selection,metrics).into_iter().zip(expected) {
                assert_eq!(grip,expected_grip);assert_eq!(center(handle),point);
                assert_eq!((handle.width(),handle.height()),(metrics.handle_size,metrics.handle_size));
                assert_eq!(hit_test(selection,point,screen,metrics),Hit::Grip(grip));
            }
        }
    }

    #[test]
    fn hit_padding_scales_at_150_and_200_percent() {
        let selection=rect(200,200,800,600);let screen=rect(0,0,1920,1080);
        for (dpi,size,padding) in [(144,12,9),(192,16,12)] {
            let metrics=metrics(dpi);assert_eq!(metrics.handle_size,size);assert_eq!(metrics.hit_padding,padding);
            let reach=size/2+padding;
            assert_eq!(hit_test(selection,(500,200-reach),screen,metrics),Hit::Grip(Grip::North));
            assert_eq!(hit_test(selection,(500,199-reach),screen,metrics),Hit::Outside);
            assert_eq!(hit_test(selection,(500,400),screen,metrics),Hit::Inside);
        }
    }

    #[test]
    fn overlapping_handle_tolerances_pick_nearest_center() {
        let selection=rect(100,100,110,110);let screen=rect(0,0,500,500);let metrics=metrics(192);
        for (grip,handle) in handles(selection,metrics) {
            assert_eq!(hit_test(selection,center(handle),screen,metrics),Hit::Grip(grip));
        }
        let selection=rect(100,100,120,120);
        assert_eq!(hit_test(selection,(105,100),screen,metrics),Hit::Grip(Grip::NorthWest),"等距时角点优先");
    }

    #[test]
    fn toolbar_buttons_win_over_handles_and_selected_pixels() {
        let screen=rect(0,0,200,100);let selection=screen;
        let metrics=SelectionMetrics {padding:0,toolbar_gap:0,..metrics(96)};
        let toolbar=toolbar_layout(selection,screen,metrics);
        assert_eq!(hit_test(selection,(199,99),screen,metrics),Hit::Save,"右下角控制点不能抢走按钮点击");
        assert_eq!(hit_test(selection,center(toolbar.cancel),screen,metrics),Hit::Cancel);
        assert_eq!(hit_test(selection,(toolbar.cancel.right,toolbar.cancel.top+1),screen,metrics),Hit::Toolbar);
        assert_eq!(hit_test(selection,(50,30),screen,metrics),Hit::Inside);
    }

    #[test]
    fn apply_drag_changes_only_sides_owned_by_each_of_the_eight_grips() {
        let selection=rect(100,100,300,300);let screen=rect(0,0,800,600);
        for (grip,expected) in [
            (Grip::NorthWest,rect(107,111,300,300)),(Grip::North,rect(100,111,300,300)),
            (Grip::NorthEast,rect(100,111,307,300)),(Grip::East,rect(100,100,307,300)),
            (Grip::SouthEast,rect(100,100,307,311)),(Grip::South,rect(100,100,300,311)),
            (Grip::SouthWest,rect(107,100,300,311)),(Grip::West,rect(107,100,300,300)),
        ] {assert_eq!(apply_drag(selection,grip,(7,11),screen),expected,"{grip:?}");}
    }

    #[test]
    fn resize_flips_across_opposite_edges_without_changing_the_original_anchor() {
        let selection=rect(100,100,300,300);let screen=rect(0,0,800,600);
        assert_eq!(apply_drag(selection,Grip::NorthWest,(250,260),screen),rect(300,300,350,360));
        assert_eq!(apply_drag(selection,Grip::NorthEast,(-250,260),screen),rect(50,300,100,360));
        assert_eq!(apply_drag(selection,Grip::SouthEast,(-250,-260),screen),rect(50,40,100,100));
        assert_eq!(apply_drag(selection,Grip::SouthWest,(250,-260),screen),rect(300,40,350,100));
        assert_eq!(apply_drag(selection,Grip::West,(250,0),screen),rect(300,100,350,300));
        assert_eq!(apply_drag(selection,Grip::East,(-250,0),screen),rect(50,100,100,300));
        assert_eq!(apply_drag(selection,Grip::North,(0,260),screen),rect(100,300,300,360));
        assert_eq!(apply_drag(selection,Grip::South,(0,-260),screen),rect(100,40,300,100));
        assert_eq!(apply_drag(selection,Grip::NorthWest,(20,30),screen),rect(120,130,300,300),"同次拖动反向时仍以按下矩形为基准");
    }

    #[test]
    fn move_preserves_size_and_clamps_both_axes_on_negative_coordinate_screen() {
        let screen=rect(-3840,-2160,0,0);let selection=rect(-3000,-1500,-2600,-1200);
        let moved=apply_drag(selection,Grip::Move,(i32::MAX,i32::MAX),screen);
        assert_eq!(moved,rect(-400,-300,0,0));
        assert_eq!((moved.width(),moved.height()),(400,300));
        assert_eq!(apply_drag(selection,Grip::Move,(i32::MIN,i32::MIN),screen),rect(-3840,-2160,-3440,-1860));
        assert_eq!(apply_drag(selection,Grip::Move,(100,-100),screen),selection.offset(100,-100));
    }

    #[test]
    fn resize_clamps_to_monitor_and_handles_extreme_pointer_deltas() {
        let screen=rect(-3840,0,0,2160);let selection=rect(-3000,200,-2600,500);
        assert_eq!(apply_drag(selection,Grip::NorthWest,(i32::MIN,i32::MIN),screen),rect(-3840,0,-2600,500));
        assert_eq!(apply_drag(selection,Grip::SouthEast,(i32::MAX,i32::MAX),screen),rect(-3000,200,0,2160));
        let oversized=rect(-4000,-100,500,2500);
        assert_eq!(apply_drag(oversized,Grip::Move,(0,0),screen),screen);
    }

    #[test]
    fn tiny_rectangles_stay_adjustable_but_are_not_valid_to_save() {
        let screen=rect(0,0,500,500);let metrics=metrics(192);
        for selection in [rect(100,100,100,100),rect(100,100,101,200),rect(100,100,200,101)] {
            assert!(!selection.valid());
            let toolbar=toolbar_layout(selection,screen,metrics);
            assert_eq!(hit_test(selection,center(toolbar.save),screen,metrics),Hit::Save,"禁用由调用方决定，不能误开始重选");
        }
        assert!(apply_drag(rect(100,100,100,100),Grip::SouthEast,(2,2),screen).valid());
        assert!(!apply_drag(rect(100,100,200,200),Grip::NorthWest,(100,100),screen).valid());
    }

    #[test]
    fn toolbar_uses_below_then_above_then_inside_and_keeps_right_alignment() {
        let screen=rect(0,0,500,500);let metrics=metrics(96);
        let below=toolbar_layout(rect(100,100,400,300),screen,metrics);
        assert_eq!((below.bounds.right,below.bounds.top),(400,306));
        let above=toolbar_layout(rect(100,300,400,490),screen,metrics);
        assert_eq!((above.bounds.right,above.bounds.bottom),(400,294));
        let inside=toolbar_layout(rect(10,10,490,490),screen,metrics);
        assert_eq!((inside.bounds.right,inside.bounds.bottom),(484,484));
        for toolbar in [below,above,inside] {
            assert!(contained(toolbar.bounds,screen));assert!(contained(toolbar.cancel,toolbar.bounds));assert!(contained(toolbar.save,toolbar.bounds));
            assert_eq!(toolbar.cancel.width(),80);assert_eq!(toolbar.save.width(),80);
        }
    }

    #[test]
    fn toolbar_stays_on_negative_coordinate_monitors_at_scaled_dpi() {
        let screen=rect(-3840,0,0,2160);
        for dpi in [144,192] {
            let metrics=metrics(dpi);
            for selection in [rect(-3839,10,-3800,100),rect(-100,1900,-1,2159),screen] {
                let toolbar=toolbar_layout(selection,screen,metrics);
                assert!(contained(toolbar.bounds,screen));assert!(contained(toolbar.cancel,toolbar.bounds));assert!(contained(toolbar.save,toolbar.bounds));
                assert_eq!(toolbar.cancel.width(),metrics.button_width);assert_eq!(toolbar.save.height(),metrics.button_height);
            }
        }
    }

    #[test]
    fn toolbar_compresses_safely_when_the_screen_is_smaller_than_its_buttons() {
        for (width,height) in [(100,30),(2,2),(1,1),(0,0)] {
            let screen=rect(-100,-100,-100+width,-100+height);
            let toolbar=toolbar_layout(screen,screen,metrics(192));
            assert!(contained(toolbar.bounds,screen));assert!(contained(toolbar.cancel,toolbar.bounds));assert!(contained(toolbar.save,toolbar.bounds));
            assert!(toolbar.cancel.right<=toolbar.save.left);
            assert_eq!((toolbar.bounds.width(),toolbar.bounds.height()),(width,height));
        }
    }
}
