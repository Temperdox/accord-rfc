//! # accord-client (Tauri)
//!
//! Desktop client. A thin Tauri shell hosts a React UI (in `../src`) and exposes
//! [`commands`] over IPC that drive a gRPC client to `accord-server`.
//!
//! The client can also **host its own server** in-process ([`hosting`]) and, with
//! the `mesh` feature, join an encrypted Yggdrasil overlay ([`mesh`]). Both are
//! exposed via a **dev-only menu** for testing; verbose [`logging`] captures the
//! client, the embedded server, and the mesh in one file for troubleshooting.
//!
//! Modules: [`state`] (session), [`grpc`] (helpers), [`commands`] (IPC),
//! [`hosting`] (embedded server), [`mesh`] (overlay), [`logging`].

// On Windows release builds, don't pop up a console window behind the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod at_rest;
mod commands;
#[cfg(debug_assertions)]
mod factory;
mod grpc;
mod history;
mod hosting;
mod identity;
mod logging;
mod mesh;
mod mls_persist;
mod peers;
mod settings;
mod state;
mod sync;
mod taverns;
#[cfg(test)]
mod taverns_it;
mod vault;

use state::Sessions;
use tokio::sync::Mutex;

fn main() {
    let builder = tauri::Builder::default()
        .manage(Mutex::new(Sessions::default()))
        .manage(Mutex::new(sync::Sync::default()))
        .manage(std::sync::Mutex::new(settings::Settings::default()))
        .manage(Mutex::new(hosting::LocalServer::default()))
        .manage(Mutex::new(taverns::HostedTaverns::default()))
        .manage(Mutex::new(mesh::MeshState::default()))
        .setup(|app| {
            // Start verbose logging; keep the writer guard alive for the process.
            if let Some(guard) = logging::init(app) {
                std::mem::forget(guard);
            }
            // Load persisted settings into managed state.
            let handle = app.handle().clone();
            let loaded = settings::load_from_disk(&handle);
            if let Ok(mut s) = tauri::Manager::state::<settings::SharedSettings>(&handle).lock() {
                *s = loaded;
            }
            // Auto-start the mesh node if the user enabled it (best-effort).
            let mesh_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                mesh::auto_start_if_enabled(&mesh_handle).await;
            });
            // The dev menu exists ONLY in debug builds, so it never ships and
            // never clutters the production UI.
            #[cfg(debug_assertions)]
            install_dev_menu(&*app)?;
            let _ = app;
            Ok(())
        });

    let app = register_handlers(builder)
        .build(tauri::generate_context!())
        .expect("error while building the Accord client");
    app.run(|app_handle, event| {
        // Gracefully stop any hosted taverns when the app exits so their SQLite
        // DBs flush cleanly (the home node is torn down with the process).
        if let tauri::RunEvent::Exit = event {
            tauri::async_runtime::block_on(taverns::stop_all(app_handle));
        }
    });
}

/// Register IPC commands. Dev-only commands are compiled in exclusively for debug
/// builds, so the hosting/mesh/log capabilities are unreachable in production.
fn register_handlers(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    #[cfg(debug_assertions)]
    {
        builder.invoke_handler(tauri::generate_handler![
            commands::auth::connect,
            commands::auth::register,
            commands::auth::login,
            commands::messaging::list_groups,
            commands::messaging::send_public_message,
            commands::messaging::send_private_message,
            commands::messaging::fetch_public_history,
            commands::messaging::fetch_private_history,
            commands::messaging::set_active_server,
            commands::messaging::create_channel,
            commands::messaging::delete_channel,
            commands::messaging::list_members,
            commands::messaging::get_my_permissions,
            commands::messaging::get_tavern,
            commands::messaging::update_tavern,
            commands::messaging::kick_member,
            commands::messaging::ban_member,
            commands::messaging::unban_member,
            commands::messaging::list_bans,
            commands::voice::join_voice,
            commands::voice::leave_voice,
            commands::voice::set_voice_state,
            commands::voice::send_voice_signal,
            commands::accounts::list_accounts,
            commands::contacts::my_contact_code,
            commands::contacts::add_contact,
            commands::contacts::list_contacts,
            commands::contacts::remove_contact,
            commands::contacts::set_contact_verified,
            commands::friends::send_friend_request,
            commands::friends::sync_friends,
            commands::friends::respond_friend_request,
            commands::friends::cancel_friend_request,
            commands::friends::resend_friend_request,
            commands::friends::peek_contact_code,
            commands::blocks::block_contact,
            commands::blocks::unblock_contact,
            commands::blocks::list_blocks,
            settings::get_settings,
            settings::set_encrypt_at_rest,
            settings::set_friend_request_policy,
            settings::set_rendezvous_node,
            settings::set_max_hosted_taverns,
            mesh::get_mesh_status,
            mesh::set_mesh_enabled,
            mesh::mesh_connect,
            mesh::mesh_disconnect,
            commands::mls::start_dm,
            commands::mls::open_contact_dm,
            commands::mls::list_dms,
            commands::server::host_private_server,
            commands::server::host_public_server,
            commands::server::create_invite_key,
            commands::server::decode_invite,
            commands::server::prepare_mesh,
            taverns::create_tavern,
            taverns::resume_hosted_taverns,
            hosting::is_dev_build,
            hosting::dev_start_local_server,
            hosting::dev_stop_local_server,
            mesh::dev_start_mesh,
            mesh::dev_stop_mesh,
            logging::dev_open_logs,
            logging::dev_log_dir,
        ])
    }
    #[cfg(not(debug_assertions))]
    {
        builder.invoke_handler(tauri::generate_handler![
            commands::auth::connect,
            commands::auth::register,
            commands::auth::login,
            commands::messaging::list_groups,
            commands::messaging::send_public_message,
            commands::messaging::send_private_message,
            commands::messaging::fetch_public_history,
            commands::messaging::fetch_private_history,
            commands::messaging::set_active_server,
            commands::messaging::create_channel,
            commands::messaging::delete_channel,
            commands::messaging::list_members,
            commands::messaging::get_my_permissions,
            commands::messaging::get_tavern,
            commands::messaging::update_tavern,
            commands::messaging::kick_member,
            commands::messaging::ban_member,
            commands::messaging::unban_member,
            commands::messaging::list_bans,
            commands::voice::join_voice,
            commands::voice::leave_voice,
            commands::voice::set_voice_state,
            commands::voice::send_voice_signal,
            commands::accounts::list_accounts,
            commands::contacts::my_contact_code,
            commands::contacts::add_contact,
            commands::contacts::list_contacts,
            commands::contacts::remove_contact,
            commands::contacts::set_contact_verified,
            commands::friends::send_friend_request,
            commands::friends::sync_friends,
            commands::friends::respond_friend_request,
            commands::friends::cancel_friend_request,
            commands::friends::resend_friend_request,
            commands::friends::peek_contact_code,
            commands::blocks::block_contact,
            commands::blocks::unblock_contact,
            commands::blocks::list_blocks,
            settings::get_settings,
            settings::set_encrypt_at_rest,
            settings::set_friend_request_policy,
            settings::set_rendezvous_node,
            settings::set_max_hosted_taverns,
            mesh::get_mesh_status,
            mesh::set_mesh_enabled,
            mesh::mesh_connect,
            mesh::mesh_disconnect,
            commands::mls::start_dm,
            commands::mls::open_contact_dm,
            commands::mls::list_dms,
            commands::server::host_private_server,
            commands::server::host_public_server,
            commands::server::create_invite_key,
            commands::server::decode_invite,
            commands::server::prepare_mesh,
            taverns::create_tavern,
            taverns::resume_hosted_taverns,
            hosting::is_dev_build,
        ])
    }
}

/// Build and install the dev-only **Dev** menu.
///
/// The single home for test capabilities (there is deliberately no in-app dev
/// banner): grouped submenus so new tools slot in without crowding the bar.
#[cfg(debug_assertions)]
fn install_dev_menu(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};

    let start_server =
        MenuItemBuilder::with_id("dev_start_server", "Start Local Server (LAN)").build(app)?;
    let stop_server =
        MenuItemBuilder::with_id("dev_stop_server", "Stop Local Server").build(app)?;
    let hosting_menu = SubmenuBuilder::new(app, "Hosting")
        .item(&start_server)
        .item(&stop_server)
        .build()?;

    let start_mesh = MenuItemBuilder::with_id("dev_start_mesh", "Start Mesh").build(app)?;
    let stop_mesh = MenuItemBuilder::with_id("dev_stop_mesh", "Stop Mesh").build(app)?;
    let mesh_menu = SubmenuBuilder::new(app, "Mesh")
        .item(&start_mesh)
        .item(&stop_mesh)
        .build()?;

    let open_logs = MenuItemBuilder::with_id("dev_open_logs", "Open Logs Folder").build(app)?;
    let factory_reset = MenuItemBuilder::with_id(
        "dev_factory_reset",
        "Factory Reset (wipe ALL data, relaunch)",
    )
    .build(app)?;

    let dev = SubmenuBuilder::new(app, "Dev")
        .item(&hosting_menu)
        .item(&mesh_menu)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&open_logs)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&factory_reset)
        .build()?;
    let menu = MenuBuilder::new(app).item(&dev).build()?;
    app.set_menu(menu)?;

    app.on_menu_event(move |app, event| {
        let app = app.clone();
        match event.id().as_ref() {
            "dev_start_server" => spawn(async move {
                if let Err(e) = hosting::start(&app, hosting::DEFAULT_HOST_PORT, false, false).await
                {
                    eprintln!("[dev] start local server failed: {e}");
                }
            }),
            "dev_stop_server" => spawn(async move {
                let _ = hosting::stop(&app).await;
            }),
            "dev_start_mesh" => spawn(async move {
                if let Err(e) = mesh::start(&app).await {
                    eprintln!("[dev] start mesh failed: {e}");
                }
            }),
            "dev_stop_mesh" => spawn(async move {
                let _ = mesh::stop(&app).await;
            }),
            "dev_open_logs" => {
                if let Err(e) = logging::open_logs(&app) {
                    eprintln!("[dev] open logs failed: {e}");
                }
            }
            "dev_factory_reset" => spawn(async move {
                // Restarts the app on success, so an Err is the only way back.
                if let Err(e) = factory::factory_reset(&app).await {
                    // tracing, not eprintln: must reach the log file.
                    tracing::error!("factory reset failed: {e}");
                }
            }),
            _ => {}
        }
    });
    Ok(())
}

/// Spawn a future on Tauri's async runtime (used by menu handlers).
#[cfg(debug_assertions)]
fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(fut: F) {
    tauri::async_runtime::spawn(fut);
}
