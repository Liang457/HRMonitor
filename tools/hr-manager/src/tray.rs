// src/tray.rs —— 托盘图标 + 菜单。
//
// 托盘是 manager 常驻的全部可见物（面板关掉时它还在）。图标是代码现画的
// 红心，不往仓库塞资源文件。菜单/图标事件走全局通道，gui.rs 周期轮询，
// 转成 UserEvent 交给事件循环处理。
use crate::autostart;
use crate::state::UserEvent;

pub struct Tray {
    _icon: tray_icon::TrayIcon,
    autostart_item: tray_icon::menu::CheckMenuItem,
}

pub const ID_OPEN: &str = "open";
pub const ID_RESTART: &str = "restart";
pub const ID_AUTOSTART: &str = "autostart";
pub const ID_QUIT: &str = "quit";

impl Tray {
    pub fn build() -> Result<Tray, String> {
        let menu = tray_icon::menu::Menu::new();
        let _ = menu.append(&tray_icon::menu::MenuItem::with_id(ID_OPEN, "打开面板", true, None));
        let _ = menu.append(&tray_icon::menu::MenuItem::with_id(ID_RESTART, "重启采集", true, None));
        let autostart_item = tray_icon::menu::CheckMenuItem::with_id(
            ID_AUTOSTART,
            "开机自启",
            true,
            autostart::get().is_some(),
            None,
        );
        let _ = menu.append(&autostart_item);
        let _ = menu.append(&tray_icon::menu::PredefinedMenuItem::separator());
        let _ = menu.append(&tray_icon::menu::MenuItem::with_id(ID_QUIT, "退出", true, None));

        let icon = tray_icon::TrayIconBuilder::new()
            .with_tooltip("BLE 心率 —— hr-manager")
            .with_icon(heart_icon()?)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| format!("创建托盘失败：{}", e))?;

        Ok(Tray { _icon: icon, autostart_item })
    }

    /// 轮询菜单/图标事件（gui.rs 周期调用），转成用户事件。
    pub fn poll(&self) -> Vec<UserEvent> {
        use tray_icon::TrayIconEvent;
        let mut events = Vec::new();

        while let Ok(ev) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            match ev.id().0.as_str() {
                ID_OPEN => events.push(UserEvent::OpenPanel),
                ID_RESTART => events.push(UserEvent::RestartDaemon),
                ID_AUTOSTART => events.push(UserEvent::ToggleAutostart),
                ID_QUIT => events.push(UserEvent::Quit),
                _ => {}
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            // 左键单击（松开）= 打开面板；右键留给菜单
            if let TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = ev
            {
                events.push(UserEvent::OpenPanel);
            }
        }
        events
    }

    /// 同步"开机自启"勾选状态（toggle 之后调用）
    pub fn set_autostart_checked(&self, checked: bool) {
        self.autostart_item.set_checked(checked);
    }
}

/// 用代码画一颗 32×32 红心当托盘图标。
fn heart_icon() -> Result<tray_icon::Icon, String> {
    const S: usize = 32;
    let mut rgba = vec![0u8; S * S * 4];
    for y in 0..S {
        for x in 0..S {
            // 隐式心形：(x² + y² - 1)³ - x²·y³ < 0，坐标归一到 [-1, 1] 附近
            let u = (x as f32 - 15.5) / 12.5;
            let v = (16.5 - y as f32) / 12.5;
            let a = u * u + v * v - 1.0;
            if a * a * a - u * u * v * v * v < 0.0 {
                let i = (y * S + x) * 4;
                rgba[i] = 235;
                rgba[i + 1] = 64;
                rgba[i + 2] = 92;
                rgba[i + 3] = 255;
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, S as u32, S as u32).map_err(|e| format!("生成图标失败：{}", e))
}
