//! 常驻组件按实际占位排布，并在开关变化时连续补位。
use crate::anim::{smoothstep, Spring};

pub(crate) const CLOCK: usize = 0;
pub(crate) const BATTERY: usize = 1;
pub(crate) const QUOTA: usize = 2;
pub(crate) fn quota_slot(position:usize)->usize {
    if position<3 {QUOTA+position} else {WIDTHS.len()+position-3}
}
pub(crate) const MUSIC: usize = 6;
pub(crate) const FOCUS: usize = 7;
pub(crate) const BELL: usize = 8;
pub(crate) const PET: usize = 9;
/// [二开] 工资组件与系统监控（CPU、内存两个圆环）。追加在上游的槽位之后，额外的额度圆环排在它们后面。
pub(crate) const SALARY: usize = 10;
pub(crate) const SALARY_WIDTH: f32 = 62.0;
pub(crate) const SYSTEM: usize = 11;
const WIDTHS: [f32; 12] = [44.0,20.0,27.0,27.0,27.0,22.0,14.0,crate::focus_bar::WIDTH,20.0,32.0,SALARY_WIDTH,crate::perf_ui::WIDTH];

pub(crate) const DEFAULT_ORDER: [usize;8]=[BATTERY,SYSTEM,PET,CLOCK,QUOTA,MUSIC,FOCUS,SALARY];
/// [二开] 没有电池时的默认排列（与 `Layout::with` 一致）。
pub(crate) const NO_BATTERY_ORDER: [usize;8]=[QUOTA,PET,CLOCK,SYSTEM,BATTERY,MUSIC,FOCUS,SALARY];
pub(crate) fn valid_order(order:&[usize])->bool {
    order.len()==DEFAULT_ORDER.len() && DEFAULT_ORDER.iter().all(|slot|order.iter().filter(|value|*value==slot).count()==1)
}
/// [二开] 读取保存的排列：旧版没有工资组件的排列在末尾补上工资；没有系统监控的排列把它插在电量后面（与默认排列一致）。
pub(crate) fn migrate_order(mut order:Vec<usize>)->Option<Vec<usize>> {
    if order.len()==DEFAULT_ORDER.len()-2 && !order.contains(&SALARY) && !order.contains(&SYSTEM) {order.push(SALARY);}
    if order.len()==DEFAULT_ORDER.len()-1 && !order.contains(&SYSTEM) {
        let at=order.iter().position(|slot|*slot==BATTERY).map_or(order.len(),|index|index+1);
        order.insert(at,SYSTEM);
    }
    valid_order(&order).then_some(order)
}

/// 收起状态要显示的组件。[二开] 在上游的六个开关之外加了工资与系统监控。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Parts {
    pub battery: bool,
    pub quotas: usize,
    pub music: bool,
    pub focus: bool,
    pub bell: bool,
    pub pet: bool,
    pub salary: bool,
    pub system: bool,
}

pub(crate) fn animations_enabled()->bool {
    #[cfg(windows)] {
        static ENABLED:std::sync::OnceLock<bool>=std::sync::OnceLock::new();
        *ENABLED.get_or_init(||{
            use windows::Win32::{Foundation::BOOL,UI::WindowsAndMessaging::*};
            let mut enabled=BOOL(1);
            unsafe {let _=SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION,0,Some((&mut enabled as *mut BOOL).cast()),SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));}
            enabled.as_bool()
        })
    }
    #[cfg(not(windows))] {true}
}

#[derive(Clone, Debug)]
pub(crate) struct Layout {
    pub width: f32,
    pub x: Vec<Option<f32>>,
    pub widths:Vec<f32>,
    quotas:usize,
}

impl Layout {
    pub fn new(battery:bool,quotas:usize,music:bool,focus:bool,bell:bool,pet:bool)->Self {
        Self::with(Parts{battery,quotas,music,focus,bell,pet,salary:false,system:false})
    }
    /// [二开] 带工资与系统监控的排布；`new` 是上游的六个开关的写法。
    pub fn with(parts:Parts)->Self {
        let Parts{battery,quotas,music,focus,bell,pet,salary,system}=parts;
        fn place(cursor:&mut f32,width:f32)->f32 {let x=*cursor+width/2.0;*cursor+=width+12.0;x}
        let mut widths=WIDTHS.to_vec(); widths.resize(WIDTHS.len()+quotas.saturating_sub(3),27.0);
        let mut x=vec![None;widths.len()];let mut cursor=16.0;
        let count=quotas;
        // [二开] 有电池：电量、系统监控在最左；没有电池：额度移到最左，系统监控跟在时钟后面。
        let order=if battery {[BATTERY,SYSTEM,CLOCK,QUOTA]}else{[QUOTA,CLOCK,SYSTEM,BATTERY]};
        for slot in order {
            match slot {
                BATTERY if battery=>x[slot]=Some(place(&mut cursor,WIDTHS[slot])),
                SYSTEM if system=>x[slot]=Some(place(&mut cursor,WIDTHS[slot])),
                CLOCK=>{
                    if pet {x[PET]=Some(place(&mut cursor,WIDTHS[PET]));}
                    x[slot]=Some(place(&mut cursor,WIDTHS[slot]));
                },
                QUOTA if count>0=>{
                    let width=count as f32*27.0+(count-1) as f32*7.0;
                    let left=place(&mut cursor,width)-width/2.0;
                    for i in 0..count {x[quota_slot(i)]=Some(left+13.5+i as f32*34.0);}
                },
                _=>{},
            }
        }
        for (slot,on) in [(MUSIC,music&&!bell),(FOCUS,focus),(SALARY,salary),(BELL,bell)] {
            if on {x[slot]=Some(place(&mut cursor,WIDTHS[slot]));}
        }
        let width=cursor-12.0+16.0;
        for value in x.iter_mut().flatten() {*value-=width/2.0;}
        Self {width:width.max(96.0),x,widths,quotas}
    }
    pub fn quota_slots(&self)->impl Iterator<Item=usize> { (0..self.quotas).map(quota_slot) }
    /// [二开] 岛上实际使用的排布。最终宽度超过 `limit` 时先让掉工资组件，仍放不下再让掉系统监控，其他组件位置不变。
    /// `tails` 是额度圆环额外的宽度（上游的余额显示，见 `balance_tail`）。
    pub fn fitted(parts:Parts,order:&[usize],clock_extra:f32,tails:&[(usize,f32)],limit:f32)->Self {
        let build=|salary:bool,system:bool|{
            let mut layout=Self::with(Parts{salary,system,..parts});
            layout.reorder(order);
            for &(position,extra) in tails {layout.balance_tail(position,extra);}
            if clock_extra>0.0 {layout.widen_clock(clock_extra);}
            layout
        };
        let mut layout=build(parts.salary,parts.system);
        if parts.salary && layout.width>limit {layout=build(false,parts.system);}
        if parts.system && layout.width>limit {layout=build(false,false);}
        layout
    }
    pub fn balance_tail(&mut self,position:usize,extra:f32){
        let slot=quota_slot(position);
        let Some(center)=self.x[slot] else{return;};
        for (i,x) in self.x.iter_mut().enumerate(){if i!=slot{if let Some(x)=x{*x+=if *x<center{-extra/2.0}else{extra/2.0};}}}
        self.widths[slot]+=extra;self.width+=extra;
    }
    pub fn widen_clock(&mut self, extra:f32) {
        let clock=self.x[CLOCK].unwrap();
        for (slot,x) in self.x.iter_mut().enumerate() {
            if slot!=CLOCK { if let Some(x)=x { *x+=if *x<clock {-extra/2.0}else{extra/2.0}; } }
        }
        self.width+=extra;
    }
    pub fn reorder(&mut self,order:&[usize]) {
        if !valid_order(order){return;}
        let original=self.x.clone();let mut cursor=-self.width/2.0+16.0;
        for slot in order.iter().copied().chain(std::iter::once(BELL)) {
            let slots:Vec<usize>=if slot==QUOTA{self.quota_slots().collect()}else{vec![slot]};
            let left=slots.iter().filter_map(|&i|original[i].map(|x|x-self.widths[i]/2.0)).fold(f32::INFINITY,f32::min);
            let right=slots.iter().filter_map(|&i|original[i].map(|x|x+self.widths[i]/2.0)).fold(f32::NEG_INFINITY,f32::max);
            if !left.is_finite(){continue;}
            for i in slots {if let Some(x)=original[i]{self.x[i]=Some(cursor+x-left);}}
            cursor+=right-left+12.0;
        }
    }
    /// 圆环换边经过时钟时，暂时淡出时钟，避免移动中相互遮挡。
    pub fn clearance(&self,slot:usize)->f32 {
        let Some(x)=self.x[slot] else{return 0.0;};
        let distance=self.quota_slots().filter_map(|i|self.x[i].map(|other|(other-x).abs()-(self.widths[i]-27.0)/2.0)).fold(f32::INFINITY,f32::min);
        let quota=if slot==CLOCK {smoothstep(34.0,44.0,distance)}else{smoothstep(22.0,34.0,distance)};
        quota*self.pet_clearance(slot)
    }
    pub fn pet_clearance(&self,slot:usize)->f32 {
        match (self.x[slot],self.x[PET]) {
            (Some(x),Some(pet))=>smoothstep(self.widths[slot]/2.0+14.0,self.widths[slot]/2.0+24.0,(x-pet).abs()),
            _=>1.0,
        }
    }
}

pub(crate) struct Motion {
    pub target: Layout,
    positions: Vec<Spring>,
    initialized: bool,
}
impl Default for Motion {
    fn default()->Self {Self {target:Layout::new(false,0,false,false,false,false),positions:vec![Spring::new(0.0,360.0,0.76);10],initialized:false}}
}
impl Motion {
    pub fn step(&mut self,target:Layout,dt:f32,animate:bool) {
        self.positions.resize(target.x.len(),Spring::new(0.0,360.0,0.76));
        for (i,x) in target.x.iter().enumerate() {
            if let Some(x)=x {
                let spring=&mut self.positions[i];spring.target=*x;
                if !self.initialized || self.target.x.get(i).copied().flatten().is_none() || !animate {spring.snap(*x);}else{spring.step(dt);}
            }
        }
        self.target=target;self.initialized=true;
    }
    pub fn current(&self)->Layout {
        let mut layout=self.target.clone();
        for (i,x) in layout.x.iter_mut().enumerate() {if x.is_some(){*x=Some(self.positions[i].pos);}}
        layout
    }
    pub fn position(&self,slot:usize)->f32 {self.positions[slot].pos}
    pub fn settled(&self)->bool {self.target.x.iter().enumerate().all(|(i,x)|x.is_none()||self.positions[i].settled(100.0))}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn deepseek_balance_follows_more_and_never_overlaps(){
        for order in [DEFAULT_ORDER,[FOCUS,SALARY,MUSIC,QUOTA,CLOCK,SYSTEM,PET,BATTERY]] {for quotas in 1..=6 {for battery in [false,true]{
            let mut layout=Layout::new(battery,quotas,true,true,false,true);layout.reorder(&order);layout.balance_tail(quotas-1,76.0);layout.widen_clock(40.0);
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|{let width=layout.widths[i]+if i==CLOCK{40.0}else{0.0};(x-width/2.0,x+width/2.0)})).collect();boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
            assert!(layout.width<crate::app::WIN_W);
        }}}
    }
    #[test]
    fn clock_12h_keeps_components_separate_in_custom_orders(){
        for order in [DEFAULT_ORDER,[FOCUS,SALARY,MUSIC,QUOTA,CLOCK,SYSTEM,PET,BATTERY]] {
            for quotas in 0..=5 {
                let mut layout=Layout::new(true,quotas,true,true,false,true);
                layout.reorder(&order);layout.widen_clock(40.0);
                let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|{
                    let width=layout.widths[i]+if i==CLOCK {40.0}else{0.0};
                    (x-width/2.0,x+width/2.0)
                })).collect();
                boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
                assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));
                assert!(boxes.first().unwrap().0>=-layout.width/2.0+15.9);
                assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
            }
        }
    }
    #[test]
    fn custom_order_preserves_groups_and_hidden_positions(){
        assert!(!valid_order(&[0,0,2,6,7,9]));
        assert!(!valid_order(&[0,1,2]));
        let order=[FOCUS,SALARY,MUSIC,QUOTA,CLOCK,SYSTEM,PET,BATTERY];
        for battery in [false,true]{for count in 0..=5 {
            let mut layout=Layout::new(battery,count,true,true,false,true);
            let width=layout.width;layout.reorder(&order);
            assert_eq!(layout.width,width);
            let positions:Vec<_>=order.iter().filter_map(|i|layout.x[*i]).collect();
            assert!(positions.windows(2).all(|p|p[0]<p[1]));
            if count>1 {assert_eq!(layout.x[QUOTA+1].unwrap()-layout.x[QUOTA].unwrap(),34.0);}
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
            boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));
        }}
    }
    #[test]
    fn every_component_combination_fits_without_overlaps(){
        for quotas in 0..=8 {for battery in [false,true] {for music in [false,true] {for focus in [false,true] {for bell in [false,true] {for pet in [false,true] {
            let layout=Layout::new(battery,quotas,music,focus,bell,pet);
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
            boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert_eq!(layout.quota_slots().count(),quotas);
            assert!(boxes.first().unwrap().0>=-layout.width/2.0+15.9);
            assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
            assert!(boxes.windows(2).all(|pair|pair[1].0-pair[0].1>=6.9),"组件不能挤在一起：{layout:?}");
            assert_eq!(layout.clearance(CLOCK),1.0);
        }}}}}}
    }
    #[test]
    fn replacing_battery_moves_continuously_and_supports_an_instant_state(){
        let before=Layout::new(true,1,true,false,false,true);let after=Layout::new(false,1,true,false,false,true);
        let mut motion=Motion::default();motion.step(before.clone(),0.0,true);
        motion.step(after.clone(),1.0/60.0,true);let mid=motion.current().x[QUOTA].unwrap();
        assert!(mid<before.x[QUOTA].unwrap()&&mid>after.x[QUOTA].unwrap());
        for _ in 0..120 {motion.step(after.clone(),1.0/60.0,true);}
        assert!((motion.current().x[QUOTA].unwrap()-after.x[QUOTA].unwrap()).abs()<0.01);
        motion.step(before.clone(),1.0/60.0,false);assert_eq!(motion.current().x,before.x);
    }

    // ---------------- [二开] 工资与系统监控 ----------------
    fn parts(battery:bool,quotas:usize,music:bool,focus:bool,bell:bool,pet:bool,salary:bool)->Parts {Parts{battery,quotas,music,focus,bell,pet,salary,system:false}}
    const CUSTOM:[usize;8]=[FOCUS,SALARY,MUSIC,QUOTA,CLOCK,SYSTEM,PET,BATTERY];
    fn boxes(layout:&Layout,clock_extra:f32)->Vec<(f32,f32)> {
        let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|{
            let width=layout.widths[i]+if i==CLOCK {clock_extra}else{0.0};
            (x-width/2.0,x+width/2.0)
        })).collect();
        boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
        boxes
    }
    #[test]
    fn fork_components_stay_separate_in_every_order(){
        for order in [DEFAULT_ORDER,NO_BATTERY_ORDER,CUSTOM] {
            for quotas in 0..=5 {for salary in [false,true] {for system in [false,true] {
                let mut layout=Layout::with(Parts{system,..parts(true,quotas,true,true,false,true,salary)});
                layout.reorder(&order);layout.widen_clock(40.0);
                let boxes=boxes(&layout,40.0);
                assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));
                assert!(boxes.first().unwrap().0>=-layout.width/2.0+15.9);
                assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
                let positions:Vec<_>=order.iter().filter_map(|i|layout.x[*i]).collect();
                assert!(positions.windows(2).all(|p|p[0]<p[1]),"按排列顺序：{order:?}");
            }}}
        }
    }
    #[test]
    fn saved_orders_from_before_the_fork_components_gain_them(){
        assert!(!valid_order(&[BATTERY,PET,CLOCK,QUOTA,MUSIC,FOCUS]),"缺少工资组件的旧排列需要先迁移");
        assert!(!valid_order(&[BATTERY,PET,CLOCK,QUOTA,MUSIC,FOCUS,SALARY]),"缺少系统监控的旧排列需要先迁移");
        assert_eq!(migrate_order(vec![FOCUS,MUSIC,QUOTA,CLOCK,PET,BATTERY]),Some(vec![FOCUS,MUSIC,QUOTA,CLOCK,PET,BATTERY,SYSTEM,SALARY]));
        assert_eq!(migrate_order(vec![SALARY,FOCUS,MUSIC,QUOTA,CLOCK,PET,BATTERY]),Some(vec![SALARY,FOCUS,MUSIC,QUOTA,CLOCK,PET,BATTERY,SYSTEM]));
        assert_eq!(migrate_order(vec![BATTERY,PET,CLOCK,QUOTA,MUSIC,FOCUS,SALARY]),Some(DEFAULT_ORDER.to_vec()),"系统监控插在电量后面，与默认排列一致");
        assert_eq!(migrate_order(vec![QUOTA,PET,CLOCK,BATTERY,MUSIC,FOCUS,SALARY]),Some(vec![QUOTA,PET,CLOCK,BATTERY,SYSTEM,MUSIC,FOCUS,SALARY]));
        assert_eq!(migrate_order(CUSTOM.to_vec()),Some(CUSTOM.to_vec()),"新版排列原样保留");
        assert_eq!(migrate_order(vec![FOCUS,FOCUS,QUOTA,CLOCK,PET,BATTERY]),None);
        assert_eq!(migrate_order(vec![FOCUS,SALARY,QUOTA,CLOCK,PET,BATTERY]),None);
        assert_eq!(migrate_order(vec![]),None);
    }
    #[test]
    fn salary_then_system_yield_when_the_island_is_full(){
        for quotas in 0..=8 {for battery in [false,true] {for music in [false,true] {for focus in [false,true] {for bell in [false,true] {for pet in [false,true] {for salary in [false,true] {for system in [false,true] {
            let wanted=Parts{system,..parts(battery,quotas,music,focus,bell,pet,salary)};
            let layout=Layout::fitted(wanted,&[],0.0,&[],crate::app::PW);
            let boxes=boxes(&layout,0.0);
            assert!(boxes.windows(2).all(|pair|pair[1].0-pair[0].1>=6.9),"组件不能挤在一起：{layout:?}");
            if salary && layout.x[SALARY].is_none() {
                assert!(Layout::with(wanted).width>crate::app::PW,"放得下时不能让掉工资组件");
            }
            if system && layout.x[SYSTEM].is_none() {
                assert!(layout.x[SALARY].is_none(),"先让掉工资组件，再让系统监控");
                assert!(Layout::with(Parts{salary:false,..wanted}).width>crate::app::PW,"放得下时不能让掉系统监控");
                assert_eq!(layout.x,Layout::with(Parts{salary:false,system:false,..wanted}).x,"系统监控让位后其他组件保持原位");
            }
            if system && quotas<=1 {assert!(layout.x[SYSTEM].is_some(),"常见组合必须能显示系统监控：{wanted:?}");}
            if salary && !system && quotas==0 {assert!(layout.x[SALARY].is_some(),"常见组合必须能显示工资：{wanted:?}");}
        }}}}}}}}
    }
    #[test]
    fn salary_yields_when_the_12_hour_clock_needs_the_room(){
        // 4 个圆环 + 音乐：24 小时制放得下，12 小时制时钟加宽后放不下。
        let wanted=parts(true,4,true,false,false,true,true);
        assert!(Layout::fitted(wanted,&[],0.0,&[],crate::app::PW).x[SALARY].is_some());
        assert!(Layout::fitted(wanted,&[],40.0,&[],crate::app::PW).x[SALARY].is_none());
    }
}
