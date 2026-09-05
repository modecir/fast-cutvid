use std::{
    cell::RefCell,
    collections::VecDeque,
    sync::{Mutex, OnceLock},
};

use eframe::egui;
use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObjectProtocol, Sel},
    sel,
};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{NSObject, NSString};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(isize)]
pub enum MenuCommand {
    ImportVideos = 1,
    OpenProject,
    Save,
    SaveAs,
    ExportVideo,
    ExportCuts,
    Split,
    DeleteClip,
    MoveClipLeft,
    MoveClipRight,
    ToggleMute,
    PlayPause,
    JumpBackSecond,
    JumpForwardSecond,
    PreviousFrame,
    NextFrame,
    PreviousEdit,
    NextEdit,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    ShowShortcuts,
    ShowHelp,
}

impl MenuCommand {
    fn from_tag(tag: isize) -> Option<Self> {
        Some(match tag {
            1 => Self::ImportVideos,
            2 => Self::OpenProject,
            3 => Self::Save,
            4 => Self::SaveAs,
            5 => Self::ExportVideo,
            6 => Self::ExportCuts,
            7 => Self::Split,
            8 => Self::DeleteClip,
            9 => Self::MoveClipLeft,
            10 => Self::MoveClipRight,
            11 => Self::ToggleMute,
            12 => Self::PlayPause,
            13 => Self::JumpBackSecond,
            14 => Self::JumpForwardSecond,
            15 => Self::PreviousFrame,
            16 => Self::NextFrame,
            17 => Self::PreviousEdit,
            18 => Self::NextEdit,
            19 => Self::ZoomIn,
            20 => Self::ZoomOut,
            21 => Self::ZoomFit,
            22 => Self::ShowShortcuts,
            23 => Self::ShowHelp,
            _ => return None,
        })
    }
}

static COMMANDS: Mutex<VecDeque<MenuCommand>> = Mutex::new(VecDeque::new());
static REPAINT_CONTEXT: OnceLock<egui::Context> = OnceLock::new();

thread_local! {
    /// NSMenuItem targets are weak. Keep the action target alive for the life
    /// of the main AppKit thread without claiming it is Send or Sync.
    static MENU_TARGET: RefCell<Option<Retained<MenuTarget>>> = const { RefCell::new(None) };
}

fn enqueue(command: MenuCommand) {
    if let Ok(mut commands) = COMMANDS.lock() {
        commands.push_back(command);
    }
    if let Some(ctx) = REPAINT_CONTEXT.get() {
        ctx.request_repaint();
    }
}

define_class!(
    // SAFETY: NSObject has no additional subclassing requirements and this
    // class has no Drop implementation or instance state.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(performFastCutAction:))]
        fn perform_fast_cut_action(&self, sender: &NSMenuItem) {
            if let Some(command) = MenuCommand::from_tag(sender.tag()) {
                enqueue(command);
            }
        }
    }

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for MenuTarget {}
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's initializer has the declared signature.
        unsafe { msg_send![super(this), init] }
    }
}

pub fn take_commands() -> Vec<MenuCommand> {
    COMMANDS
        .lock()
        .map(|mut commands| commands.drain(..).collect())
        .unwrap_or_default()
}

pub fn install(ctx: &egui::Context) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("macOS menu must be installed on the main thread");
        return;
    };
    let _ = REPAINT_CONTEXT.set(ctx.clone());

    let target = MenuTarget::new(mtm);
    let app = NSApplication::sharedApplication(mtm);
    let main_menu = menu("fastCutVid", mtm);

    let application_menu = menu("fastCutVid", mtm);
    add_system_item(
        &application_menu,
        "About fastCutVid",
        "",
        NSEventModifierFlags::empty(),
        sel!(orderFrontStandardAboutPanel:),
        Some(&app),
        mtm,
    );
    application_menu.addItem(&NSMenuItem::separatorItem(mtm));

    let services_menu = menu("Services", mtm);
    let services_item = plain_item("Services", mtm);
    services_item.setSubmenu(Some(&services_menu));
    application_menu.addItem(&services_item);
    app.setServicesMenu(Some(&services_menu));

    application_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_system_item(
        &application_menu,
        "Hide fastCutVid",
        "h",
        NSEventModifierFlags::Command,
        sel!(hide:),
        Some(&app),
        mtm,
    );
    add_system_item(
        &application_menu,
        "Hide Others",
        "h",
        NSEventModifierFlags::Command | NSEventModifierFlags::Option,
        sel!(hideOtherApplications:),
        Some(&app),
        mtm,
    );
    add_system_item(
        &application_menu,
        "Show All",
        "",
        NSEventModifierFlags::empty(),
        sel!(unhideAllApplications:),
        Some(&app),
        mtm,
    );
    application_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_system_item(
        &application_menu,
        "Quit fastCutVid",
        "q",
        NSEventModifierFlags::Command,
        sel!(terminate:),
        Some(&app),
        mtm,
    );
    add_submenu(&main_menu, "fastCutVid", &application_menu, mtm);

    let file_menu = menu("File", mtm);
    add_command_item(
        &file_menu,
        "Import Videos…",
        "i",
        NSEventModifierFlags::Command,
        MenuCommand::ImportVideos,
        &target,
        mtm,
    );
    add_command_item(
        &file_menu,
        "Open Project…",
        "o",
        NSEventModifierFlags::Command,
        MenuCommand::OpenProject,
        &target,
        mtm,
    );
    file_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_command_item(
        &file_menu,
        "Save",
        "s",
        NSEventModifierFlags::Command,
        MenuCommand::Save,
        &target,
        mtm,
    );
    add_command_item(
        &file_menu,
        "Save As…",
        "s",
        NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        MenuCommand::SaveAs,
        &target,
        mtm,
    );
    file_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_command_item(
        &file_menu,
        "Export Video…",
        "e",
        NSEventModifierFlags::Command,
        MenuCommand::ExportVideo,
        &target,
        mtm,
    );
    add_command_item(
        &file_menu,
        "Export Cuts…",
        "e",
        NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        MenuCommand::ExportCuts,
        &target,
        mtm,
    );
    add_submenu(&main_menu, "File", &file_menu, mtm);

    let edit_menu = menu("Edit", mtm);
    add_command_item(
        &edit_menu,
        "Split at Playhead",
        "k",
        NSEventModifierFlags::Command,
        MenuCommand::Split,
        &target,
        mtm,
    );
    add_command_item(
        &edit_menu,
        "Delete Selected Clip",
        "\u{7f}",
        NSEventModifierFlags::empty(),
        MenuCommand::DeleteClip,
        &target,
        mtm,
    );
    edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_command_item(
        &edit_menu,
        "Move Clip Left",
        "\u{f702}",
        NSEventModifierFlags::Option,
        MenuCommand::MoveClipLeft,
        &target,
        mtm,
    );
    add_command_item(
        &edit_menu,
        "Move Clip Right",
        "\u{f703}",
        NSEventModifierFlags::Option,
        MenuCommand::MoveClipRight,
        &target,
        mtm,
    );
    add_command_item(
        &edit_menu,
        "Mute / Unmute Clip",
        "m",
        NSEventModifierFlags::empty(),
        MenuCommand::ToggleMute,
        &target,
        mtm,
    );
    add_submenu(&main_menu, "Edit", &edit_menu, mtm);

    let playback_menu = menu("Playback", mtm);
    add_command_item(
        &playback_menu,
        "Play / Pause",
        " ",
        NSEventModifierFlags::empty(),
        MenuCommand::PlayPause,
        &target,
        mtm,
    );
    playback_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_command_item(
        &playback_menu,
        "Jump Back One Second",
        "\u{f702}",
        NSEventModifierFlags::empty(),
        MenuCommand::JumpBackSecond,
        &target,
        mtm,
    );
    add_command_item(
        &playback_menu,
        "Jump Forward One Second",
        "\u{f703}",
        NSEventModifierFlags::empty(),
        MenuCommand::JumpForwardSecond,
        &target,
        mtm,
    );
    add_command_item(
        &playback_menu,
        "Previous Frame",
        "\u{f702}",
        NSEventModifierFlags::Shift,
        MenuCommand::PreviousFrame,
        &target,
        mtm,
    );
    add_command_item(
        &playback_menu,
        "Next Frame",
        "\u{f703}",
        NSEventModifierFlags::Shift,
        MenuCommand::NextFrame,
        &target,
        mtm,
    );
    playback_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_command_item(
        &playback_menu,
        "Previous Edit",
        "\u{f700}",
        NSEventModifierFlags::empty(),
        MenuCommand::PreviousEdit,
        &target,
        mtm,
    );
    add_command_item(
        &playback_menu,
        "Next Edit",
        "\u{f701}",
        NSEventModifierFlags::empty(),
        MenuCommand::NextEdit,
        &target,
        mtm,
    );
    add_submenu(&main_menu, "Playback", &playback_menu, mtm);

    let view_menu = menu("View", mtm);
    add_command_item(
        &view_menu,
        "Zoom In",
        "+",
        NSEventModifierFlags::Command,
        MenuCommand::ZoomIn,
        &target,
        mtm,
    );
    add_command_item(
        &view_menu,
        "Zoom Out",
        "-",
        NSEventModifierFlags::Command,
        MenuCommand::ZoomOut,
        &target,
        mtm,
    );
    add_command_item(
        &view_menu,
        "Fit Timeline",
        "0",
        NSEventModifierFlags::Command,
        MenuCommand::ZoomFit,
        &target,
        mtm,
    );
    add_submenu(&main_menu, "View", &view_menu, mtm);

    let window_menu = menu("Window", mtm);
    add_system_item(
        &window_menu,
        "Minimize",
        "m",
        NSEventModifierFlags::Command,
        sel!(performMiniaturize:),
        None,
        mtm,
    );
    add_system_item(
        &window_menu,
        "Zoom",
        "",
        NSEventModifierFlags::empty(),
        sel!(performZoom:),
        None,
        mtm,
    );
    window_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_system_item(
        &window_menu,
        "Bring All to Front",
        "",
        NSEventModifierFlags::empty(),
        sel!(arrangeInFront:),
        None,
        mtm,
    );
    add_submenu(&main_menu, "Window", &window_menu, mtm);
    app.setWindowsMenu(Some(&window_menu));

    let help_menu = menu("Help", mtm);
    add_command_item(
        &help_menu,
        "fastCutVid Quick Start",
        "\u{f704}",
        NSEventModifierFlags::empty(),
        MenuCommand::ShowHelp,
        &target,
        mtm,
    );
    add_command_item(
        &help_menu,
        "Keyboard Shortcuts",
        "/",
        NSEventModifierFlags::Shift,
        MenuCommand::ShowShortcuts,
        &target,
        mtm,
    );
    add_submenu(&main_menu, "Help", &help_menu, mtm);
    app.setHelpMenu(Some(&help_menu));

    app.setMainMenu(Some(&main_menu));
    MENU_TARGET.with(|stored| *stored.borrow_mut() = Some(target));
}

fn menu(title: &str, mtm: MainThreadMarker) -> Retained<NSMenu> {
    NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title))
}

fn plain_item(title: &str, mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    // SAFETY: No selector is supplied, and both NSString arguments remain
    // valid for the duration of initialization.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            None,
            &NSString::from_str(""),
        )
    }
}

fn add_submenu(root: &NSMenu, title: &str, submenu: &NSMenu, mtm: MainThreadMarker) {
    let item = plain_item(title, mtm);
    item.setSubmenu(Some(submenu));
    root.addItem(&item);
}

#[allow(clippy::too_many_arguments)]
fn add_command_item(
    menu: &NSMenu,
    title: &str,
    key: &str,
    modifiers: NSEventModifierFlags,
    command: MenuCommand,
    target: &MenuTarget,
    mtm: MainThreadMarker,
) {
    let item = action_item(
        title,
        key,
        modifiers,
        sel!(performFastCutAction:),
        Some(target),
        mtm,
    );
    item.setTag(command as isize);
    menu.addItem(&item);
}

#[allow(clippy::too_many_arguments)]
fn add_system_item(
    menu: &NSMenu,
    title: &str,
    key: &str,
    modifiers: NSEventModifierFlags,
    action: Sel,
    target: Option<&AnyObject>,
    mtm: MainThreadMarker,
) {
    let item = action_item(title, key, modifiers, action, target, mtm);
    menu.addItem(&item);
}

fn action_item(
    title: &str,
    key: &str,
    modifiers: NSEventModifierFlags,
    action: Sel,
    target: Option<&AnyObject>,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    // SAFETY: Every selector is implemented by either MenuTarget,
    // NSApplication, or the standard AppKit responder chain. Targets are kept
    // alive by NSApplication or MENU_TARGET.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    };
    item.setKeyEquivalentModifierMask(modifiers);
    unsafe { item.setTarget(target) };
    item
}
