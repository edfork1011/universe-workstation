#include <gtk/gtk.h>
#include <cairo.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include "ws_version.h"

#define MAX_PAIRED_CARDS 32

typedef struct {
    void (*on_startup_ready)(void *user_data);
    void (*on_pair_start)(const char *device_id, void *user_data);
    void (*on_pair_approve)(const char *session_id, void *user_data);
    void (*on_pair_reject)(const char *session_id, void *user_data);
    void (*on_unpair)(const char *device_id, void *user_data);
    void (*on_set_sync_paused)(int paused, void *user_data);
    void (*on_send_files)(const char *peer_id, const char **paths, int count, void *user_data);
    void (*on_open_downloads)(void *user_data);
    void (*on_set_user_name)(const char *name, void *user_data);
    void (*on_set_device_name)(const char *name, void *user_data);
    void (*on_quit_clicked)(void *user_data);
} WsUiCallbacks;

typedef enum {
    PAGE_DEVICES = 0,
    PAGE_CLIPBOARD = 1,
    PAGE_TRANSFERS = 2,
    PAGE_SCREEN = 3,
    PAGE_NOTES = 4,
} WsPage;

typedef enum {
    MODAL_NONE = 0,
    MODAL_SETTINGS = 1,
    MODAL_DISCOVER = 2,
} WsModalKind;

typedef enum {
    THEME_LIGHT = 0,
    THEME_DARK = 1,
    THEME_SYSTEM = 2,
} WsThemeMode;

typedef enum {
    ICON_DEVICES = 0,
    ICON_CLIPBOARD,
    ICON_TRANSFER,
    ICON_SCREEN,
    ICON_NOTES,
    ICON_SETTINGS,
    ICON_PLUS,
    ICON_CLOSE,
    ICON_SEND,
    ICON_FOLDER,
    ICON_HIDE,
} WsIconKind;

typedef struct {
    GtkApplication *app;
    GtkWidget *window;
    GtkWidget *root_overlay;
    GtkWidget *stage_overlay;
    GtkWidget *main_hbox;
    GtkWidget *content_stack;
    GtkWidget *pages[5];

    GtkWidget *nav_buttons[5];
    GtkWidget *settings_nav_btn;
    GtkWidget *fab_btn;

    /* Page 0: Devices */
    GtkWidget *greeting_label;
    GtkWidget *lan_status_badge;
    GtkWidget *sas_revealer;
    GtkWidget *sas_container;
    GtkWidget *paired_flow;
    GtkWidget *paired_empty_card;
    GtkWidget *paired_hero_labels[MAX_PAIRED_CARDS];
    GtkWidget *paired_cap_labels[MAX_PAIRED_CARDS];
    int paired_count;

    /* Page 1: Clipboard */
    GtkWidget *clip_switch;
    GtkWidget *clip_state_badge;
    GtkWidget *clip_summary_box;

    /* Page 2: Transfers */
    GtkWidget *receive_dir_label;
    GtkWidget *quick_send_bar;
    GtkWidget *transfers_list_box;
    GtkWidget *transfers_empty_card;
    int transfer_count;
    int online_peer_count;

    /* Modal */
    GtkWidget *modal_scrim_box;
    GtkWidget *modal_scrim_backdrop;
    GtkWidget *modal_card;
    GtkWidget *modal_title_label;
    GtkWidget *modal_body_stack;

    /* Settings modal widgets */
    GtkWidget *user_name_entry;
    GtkWidget *device_name_entry;
    GtkWidget *theme_btns[3];
    GtkWidget *settings_clip_switch;
    GtkWidget *settings_dir_label;
    GtkWidget *settings_peers_box;
    GtkWidget *settings_peers_empty;
    GtkWidget *local_device_label;
    GtkWidget *local_fp_label;
    GtkWidget *local_port_label;

    /* Discover modal widgets */
    GtkWidget *discover_list_box;
    GtkWidget *discover_empty_card;
    int discovered_count;

    /* Theme / visual state */
    GtkCssProvider *css_provider;
    cairo_surface_t *logo_surface;
    int is_dark;
    WsThemeMode theme_mode;
    WsPage active_page;
    WsModalKind modal_kind;
    int nav_initialized;
    guint modal_open_source_id;
    guint modal_close_source_id;
    guint win_anim_source_id;

    int start_silent;
    int sync_paused;
    int updating_sync_switch;
    int quit_requested;
    char *display_user;
    char *current_device_name;

    WsUiCallbacks cb;
    void *user_data;
} WsUiState;

static void cancel_ui_timers(WsUiState *st) {
    if (!st) return;
    if (st->modal_open_source_id) {
        g_source_remove(st->modal_open_source_id);
        st->modal_open_source_id = 0;
    }
    if (st->modal_close_source_id) {
        g_source_remove(st->modal_close_source_id);
        st->modal_close_source_id = 0;
    }
    if (st->win_anim_source_id) {
        g_source_remove(st->win_anim_source_id);
        st->win_anim_source_id = 0;
    }
}

typedef struct {
    WsUiState *st;
    char *id;
} IdActionData;

static void id_action_free(gpointer data, GClosure *closure) {
    (void)closure;
    IdActionData *d = (IdActionData *)data;
    if (d) {
        g_free(d->id);
        g_free(d);
    }
}

static IdActionData *id_action_new(WsUiState *st, const char *id) {
    IdActionData *d = g_new0(IdActionData, 1);
    d->st = st;
    d->id = g_strdup(id ? id : "");
    return d;
}

static void clear_container(GtkWidget *box) {
    if (!box) return;
    GtkWidget *child = gtk_widget_get_first_child(box);
    while (child) {
        GtkWidget *next = gtk_widget_get_next_sibling(child);
        if (GTK_IS_FLOW_BOX(box)) {
            gtk_flow_box_remove(GTK_FLOW_BOX(box), child);
        } else {
            gtk_box_remove(GTK_BOX(box), child);
        }
        child = next;
    }
}

/* ---------------- Translucent Liquid Glass CSS (GTK 4.14+ NGL GPU) ---------------- */

static const char *GLASS_CSS =
"window.glass-win {\n"
"  background-color: transparent;\n"
"  background-image: none;\n"
"}\n"
"@keyframes page-glide-down {\n"
"  from { opacity: 0; transform: translateY(14px) scale(0.992); }\n"
"  to { opacity: 1; transform: translateY(0px) scale(1); }\n"
"}\n"
"@keyframes page-glide-up {\n"
"  from { opacity: 0; transform: translateY(-14px) scale(0.992); }\n"
"  to { opacity: 1; transform: translateY(0px) scale(1); }\n"
"}\n"
"@keyframes card-enter {\n"
"  from { opacity: 0; transform: translateY(10px) scale(0.985); }\n"
"  to { opacity: 1; transform: translateY(0px) scale(1); }\n"
"}\n"
"@keyframes win-present {\n"
"  from { opacity: 0; transform: scale(0.975); }\n"
"  to { opacity: 1; transform: scale(1); }\n"
"}\n"
".page-enter-down {\n"
"  animation-name: page-glide-down;\n"
"  animation-duration: 230ms;\n"
"  animation-timing-function: cubic-bezier(0.22, 1, 0.36, 1);\n"
"  animation-iteration-count: 1;\n"
"}\n"
".page-enter-up {\n"
"  animation-name: page-glide-up;\n"
"  animation-duration: 230ms;\n"
"  animation-timing-function: cubic-bezier(0.22, 1, 0.36, 1);\n"
"  animation-iteration-count: 1;\n"
"}\n"
".card-enter {\n"
"  animation-name: card-enter;\n"
"  animation-duration: 220ms;\n"
"  animation-timing-function: cubic-bezier(0.22, 1, 0.36, 1);\n"
"  animation-iteration-count: 1;\n"
"}\n"
".win-presenting {\n"
"  animation-name: win-present;\n"
"  animation-duration: 190ms;\n"
"  animation-timing-function: cubic-bezier(0.22, 1, 0.36, 1);\n"
"  animation-iteration-count: 1;\n"
"}\n"
".glass-shell {\n"
"  border-radius: 24px;\n"
"  border: 1px solid rgba(255, 255, 255, 0.82);\n"
"  background-color: transparent;\n"
"  backdrop-filter: blur(28px);\n"
"  background-image:\n"
"    radial-gradient(circle at 16% 14%, rgba(96, 165, 250, 0.16), rgba(96, 165, 250, 0.0) 46%),\n"
"    radial-gradient(circle at 86% 84%, rgba(52, 211, 153, 0.12), rgba(52, 211, 153, 0.0) 48%),\n"
"    linear-gradient(145deg, rgba(244, 247, 253, 0.75), rgba(232, 238, 248, 0.69));\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.92);\n"
"  color: #0f172a;\n"
"  transition: background-color 220ms ease, background-image 220ms ease, border-color 220ms ease, box-shadow 220ms ease, color 180ms ease;\n"
"}\n"
"window.theme-dark .glass-shell {\n"
"  border: 1px solid rgba(255, 255, 255, 0.20);\n"
"  background-image:\n"
"    radial-gradient(circle at 16% 14%, rgba(59, 130, 246, 0.18), rgba(59, 130, 246, 0.0) 46%),\n"
"    radial-gradient(circle at 86% 84%, rgba(20, 184, 166, 0.14), rgba(20, 184, 166, 0.0) 48%),\n"
"    linear-gradient(145deg, rgba(22, 27, 40, 0.78), rgba(15, 19, 30, 0.73));\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.18);\n"
"  color: #f1f5f9;\n"
"}\n"
".stage-blurred {\n"
"  filter: blur(12px);\n"
"}\n"
".glass-sidebar {\n"
"  padding: 20px 14px 16px 14px;\n"
"  border-radius: 24px 0 0 24px;\n"
"  background-color: transparent;\n"
"  background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.62), rgba(240, 244, 252, 0.48));\n"
"  border-right: 1px solid rgba(15, 23, 42, 0.10);\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.88);\n"
"  transition: background-color 220ms ease, background-image 220ms ease, border-color 220ms ease, box-shadow 220ms ease, color 180ms ease;\n"
"}\n"
"window.theme-dark .glass-sidebar {\n"
"  background-image: linear-gradient(180deg, rgba(32, 39, 56, 0.66), rgba(20, 25, 38, 0.54));\n"
"  border-right: 1px solid rgba(255, 255, 255, 0.11);\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.14);\n"
"}\n"
".brand-title {\n"
"  font-size: 16px;\n"
"  font-weight: 800;\n"
"  letter-spacing: -0.2px;\n"
"}\n"
".brand-sub {\n"
"  font-size: 10px;\n"
"  font-weight: 500;\n"
"  color: rgba(15, 23, 42, 0.66);\n"
"}\n"
"window.theme-dark .brand-sub {\n"
"  color: rgba(241, 245, 249, 0.66);\n"
"}\n"
"button.nav-btn {\n"
"  border-radius: 15px;\n"
"  padding: 0 12px;\n"
"  border: 1px solid rgba(15, 23, 42, 0.16);\n"
"  background-color: transparent;\n"
"  background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.58), rgba(255, 255, 255, 0.36));\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.72);\n"
"  color: #0f172a;\n"
"  font-size: 13px;\n"
"  font-weight: 600;\n"
"  transform: translateX(0px) scale(1);\n"
"  transition: background-color 180ms cubic-bezier(0.22, 1, 0.36, 1), background-image 180ms cubic-bezier(0.22, 1, 0.36, 1), border-color 180ms cubic-bezier(0.22, 1, 0.36, 1), box-shadow 200ms cubic-bezier(0.22, 1, 0.36, 1), color 160ms ease, transform 160ms cubic-bezier(0.22, 1, 0.36, 1);\n"
"}\n"
"button.nav-btn:hover {\n"
"  border-color: rgba(255, 255, 255, 0.85);\n"
"  background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.72), rgba(248, 250, 255, 0.50));\n"
"  transform: translateX(2px);\n"
"}\n"
"button.nav-btn.nav-active {\n"
"  border: 1px solid rgba(255, 255, 255, 0.95);\n"
"  background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.92), rgba(244, 249, 255, 0.76));\n"
"  box-shadow: inset 0 1px 0 #ffffff,\n"
"              0 6px 16px rgba(15, 23, 42, 0.10);\n"
"  color: #0e1118;\n"
"  transform: translateX(3px);\n"
"}\n"
"button.nav-btn:active {\n"
"  transform: scale(0.975);\n"
"}\n"
"window.theme-dark button.nav-btn {\n"
"  border: 1px solid rgba(255, 255, 255, 0.12);\n"
"  background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.08), rgba(255, 255, 255, 0.03));\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.10);\n"
"  color: rgba(241, 245, 249, 0.82);\n"
"}\n"
"window.theme-dark button.nav-btn:hover {\n"
"  border-color: rgba(255, 255, 255, 0.24);\n"
"  background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.16), rgba(255, 255, 255, 0.08));\n"
"}\n"
"window.theme-dark button.nav-btn.nav-active {\n"
"  border: 1px solid rgba(255, 255, 255, 0.30);\n"
"  background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.24), rgba(255, 255, 255, 0.12));\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.26),\n"
"              0 6px 18px rgba(0, 0, 0, 0.32);\n"
"  color: #ffffff;\n"
"}\n"
".main-stage {\n"
"  padding: 18px 28px 26px 28px;\n"
"  background-color: transparent;\n"
"}\n"
"button.win-hide-btn {\n"
"  min-width: 30px;\n"
"  min-height: 30px;\n"
"  padding: 0;\n"
"  border-radius: 999px;\n"
"  border: 1px solid rgba(15, 23, 42, 0.10);\n"
"  background-color: rgba(255, 255, 255, 0.56);\n"
"  background-image: none;\n"
"  color: rgba(18, 21, 30, 0.72);\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.85);\n"
"  transform: scale(1);\n"
"  transition: background-color 160ms ease, border-color 160ms ease, color 160ms ease, transform 150ms cubic-bezier(0.22, 1, 0.36, 1);\n"
"}\n"
"button.win-hide-btn:hover {\n"
"  background-color: rgba(255, 255, 255, 0.90);\n"
"  color: #111319;\n"
"  transform: scale(1.06);\n"
"}\n"
"button.win-hide-btn:active {\n"
"  transform: scale(0.94);\n"
"}\n"
"window.theme-dark button.win-hide-btn {\n"
"  border: 1px solid rgba(255, 255, 255, 0.16);\n"
"  background-color: rgba(255, 255, 255, 0.10);\n"
"  color: rgba(241, 245, 249, 0.76);\n"
"  box-shadow: none;\n"
"}\n"
"window.theme-dark button.win-hide-btn:hover {\n"
"  background-color: rgba(255, 255, 255, 0.20);\n"
"  color: #ffffff;\n"
"}\n"
".hero-title {\n"
"  font-size: 30px;\n"
"  font-weight: 800;\n"
"  letter-spacing: -0.6px;\n"
"}\n"
".hero-sub {\n"
"  font-size: 13px;\n"
"  color: rgba(15, 23, 42, 0.72);\n"
"}\n"
"window.theme-dark .hero-sub {\n"
"  color: rgba(241, 245, 249, 0.72);\n"
"}\n"
".section-title {\n"
"  font-size: 14px;\n"
"  font-weight: 700;\n"
"  color: rgba(15, 23, 42, 0.90);\n"
"}\n"
"window.theme-dark .section-title {\n"
"  color: rgba(241, 245, 249, 0.92);\n"
"}\n"
".glass-card {\n"
"  border-radius: 22px;\n"
"  padding: 20px 22px;\n"
"  border: 1px solid rgba(255, 255, 255, 0.90);\n"
"  background-color: transparent;\n"
"  background-image: linear-gradient(145deg, rgba(255, 255, 255, 0.80), rgba(246, 249, 255, 0.64));\n"
"  box-shadow: inset 0 1px 0 #ffffff,\n"
"              0 10px 26px rgba(15, 23, 42, 0.08);\n"
"  transition: background-color 220ms ease, background-image 220ms ease, border-color 220ms ease, box-shadow 220ms ease, color 180ms ease;\n"
"}\n"
"window.theme-dark .glass-card {\n"
"  border: 1px solid rgba(255, 255, 255, 0.18);\n"
"  background-image: linear-gradient(145deg, rgba(40, 48, 68, 0.74), rgba(26, 32, 46, 0.60));\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.14),\n"
"              0 12px 30px rgba(0, 0, 0, 0.32);\n"
"}\n"
".device-card {\n"
"  transform: translateY(0px);\n"
"  transition: transform 200ms cubic-bezier(0.22, 1, 0.36, 1), border-color 200ms ease, box-shadow 220ms ease, background-image 220ms ease;\n"
"}\n"
".device-card:hover {\n"
"  transform: translateY(-3px);\n"
"  border-color: rgba(255, 255, 255, 0.96);\n"
"  box-shadow: inset 0 1px 0 #ffffff,\n"
"              0 14px 32px rgba(15, 23, 42, 0.12);\n"
"}\n"
"window.theme-dark .device-card:hover {\n"
"  border-color: rgba(255, 255, 255, 0.28);\n"
"}\n"
".card-title {\n"
"  font-size: 17px;\n"
"  font-weight: 700;\n"
"}\n"
".mono-meta {\n"
"  font-family: monospace;\n"
"  font-size: 11px;\n"
"  color: rgba(15, 23, 42, 0.68);\n"
"}\n"
"window.theme-dark .mono-meta {\n"
"  color: rgba(241, 245, 249, 0.68);\n"
"}\n"
".status-pill {\n"
"  border-radius: 999px;\n"
"  padding: 3px 10px;\n"
"  font-size: 11px;\n"
"  font-weight: 700;\n"
"  transition: background-color 220ms ease, background-image 220ms ease, border-color 220ms ease, box-shadow 220ms ease, color 180ms ease;\n"
"}\n"
".pill-online {\n"
"  background-color: rgba(74, 222, 128, 0.26);\n"
"  border: 1px solid rgba(34, 197, 94, 0.36);\n"
"  color: #15803d;\n"
"}\n"
"window.theme-dark .pill-online {\n"
"  background-color: rgba(74, 222, 128, 0.20);\n"
"  color: #6ee7b7;\n"
"}\n"
".pill-offline {\n"
"  background-color: rgba(100, 116, 139, 0.16);\n"
"  border: 1px solid rgba(100, 116, 139, 0.24);\n"
"  color: #475569;\n"
"}\n"
"window.theme-dark .pill-offline {\n"
"  background-color: rgba(148, 163, 184, 0.18);\n"
"  color: #cbd5e1;\n"
"}\n"
".pill-amber {\n"
"  background-color: rgba(245, 158, 11, 0.20);\n"
"  border: 1px solid rgba(245, 158, 11, 0.36);\n"
"  color: #b45309;\n"
"}\n"
"window.theme-dark .pill-amber {\n"
"  color: #fcd34d;\n"
"}\n"
".pill-info {\n"
"  background-color: rgba(255, 255, 255, 0.62);\n"
"  border: 1px solid rgba(15, 23, 42, 0.08);\n"
"  color: rgba(18, 21, 30, 0.76);\n"
"}\n"
"window.theme-dark .pill-info {\n"
"  background-color: rgba(255, 255, 255, 0.10);\n"
"  border: 1px solid rgba(255, 255, 255, 0.16);\n"
"  color: rgba(241, 245, 249, 0.84);\n"
"}\n"
".pill-danger {\n"
"  background-color: rgba(239, 68, 68, 0.16);\n"
"  border: 1px solid rgba(239, 68, 68, 0.30);\n"
"  color: #dc2626;\n"
"}\n"
".metric-hero {\n"
"  font-size: 46px;\n"
"  font-weight: 800;\n"
"  letter-spacing: -1.5px;\n"
"}\n"
".metric-fast {\n"
"  color: rgba(22, 163, 74, 0.78);\n"
"}\n"
"window.theme-dark .metric-fast {\n"
"  color: rgba(110, 231, 183, 0.84);\n"
"}\n"
".metric-warm {\n"
"  color: rgba(234, 179, 8, 0.86);\n"
"}\n"
"window.theme-dark .metric-warm {\n"
"  color: rgba(253, 224, 71, 0.88);\n"
"}\n"
".metric-slow {\n"
"  color: rgba(244, 63, 94, 0.82);\n"
"}\n"
".metric-muted {\n"
"  color: rgba(100, 116, 139, 0.48);\n"
"}\n"
"window.theme-dark .metric-muted {\n"
"  color: rgba(148, 163, 184, 0.44);\n"
"}\n"
".metric-caption {\n"
"  font-size: 11px;\n"
"  font-weight: 600;\n"
"  color: rgba(15, 23, 42, 0.65);\n"
"}\n"
"window.theme-dark .metric-caption {\n"
"  color: rgba(241, 245, 249, 0.65);\n"
"}\n"
"button.glass-btn {\n"
"  border-radius: 999px;\n"
"  padding: 6px 14px;\n"
"  font-size: 12px;\n"
"  font-weight: 600;\n"
"  border: 1px solid rgba(15, 23, 42, 0.12);\n"
"  background-color: transparent;\n"
"  background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.90), rgba(242, 246, 254, 0.72));\n"
"  box-shadow: inset 0 1px 0 #ffffff,\n"
"              0 4px 12px rgba(15, 23, 42, 0.06);\n"
"  color: #161820;\n"
"  transform: translateY(0px) scale(1);\n"
"  transition: background-image 170ms cubic-bezier(0.22, 1, 0.36, 1), border-color 170ms ease, box-shadow 170ms ease, transform 170ms cubic-bezier(0.22, 1, 0.36, 1);\n"
"}\n"
"button.glass-btn:hover {\n"
"  background-image: linear-gradient(180deg, #ffffff, #f6f9ff);\n"
"  transform: translateY(-1px);\n"
"}\n"
"button.glass-btn:active {\n"
"  transform: translateY(0px) scale(0.96);\n"
"}\n"
"window.theme-dark button.glass-btn {\n"
"  border: 1px solid rgba(255, 255, 255, 0.20);\n"
"  background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.18), rgba(255, 255, 255, 0.09));\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.18);\n"
"  color: #eef1f7;\n"
"}\n"
"window.theme-dark button.glass-btn:hover {\n"
"  background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.28), rgba(255, 255, 255, 0.16));\n"
"}\n"
"button.glass-btn-primary {\n"
"  border-radius: 999px;\n"
"  padding: 8px 16px;\n"
"  font-size: 12px;\n"
"  font-weight: 700;\n"
"  border: 1px solid rgba(255, 255, 255, 0.30);\n"
"  background-color: transparent;\n"
"  background-image: linear-gradient(180deg, #3b82f6, #2563eb);\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.35),\n"
"              0 6px 18px rgba(37, 99, 235, 0.30);\n"
"  color: #ffffff;\n"
"  transform: translateY(0px) scale(1);\n"
"  transition: background-image 170ms cubic-bezier(0.22, 1, 0.36, 1), box-shadow 170ms ease, transform 170ms cubic-bezier(0.22, 1, 0.36, 1);\n"
"}\n"
"button.glass-btn-primary:hover {\n"
"  background-image: linear-gradient(180deg, #4f8ff8, #2d6df0);\n"
"  transform: translateY(-1px);\n"
"}\n"
"button.glass-btn-primary:active {\n"
"  transform: translateY(0px) scale(0.96);\n"
"}\n"
"button.glass-btn-danger {\n"
"  border-radius: 999px;\n"
"  padding: 7px 14px;\n"
"  font-size: 12px;\n"
"  font-weight: 700;\n"
"  border: 1px solid rgba(239, 68, 68, 0.28);\n"
"  background-image: none;\n"
"  background-color: rgba(239, 68, 68, 0.14);\n"
"  color: #dc2626;\n"
"  transform: translateY(0px) scale(1);\n"
"  transition: background-color 170ms ease, border-color 170ms ease, transform 170ms cubic-bezier(0.22, 1, 0.36, 1);\n"
"}\n"
"button.glass-btn-danger:hover {\n"
"  background-color: rgba(239, 68, 68, 0.24);\n"
"  transform: translateY(-1px);\n"
"}\n"
"button.glass-btn-danger:active {\n"
"  transform: translateY(0px) scale(0.96);\n"
"}\n"
"window.theme-dark button.glass-btn-danger {\n"
"  color: #fca5a5;\n"
"}\n"
"button.fab-btn {\n"
"  min-width: 56px;\n"
"  min-height: 56px;\n"
"  padding: 0;\n"
"  border-radius: 999px;\n"
"  border: 1px solid rgba(255, 255, 255, 0.90);\n"
"  background-color: transparent;\n"
"  background-image: linear-gradient(160deg, rgba(255, 255, 255, 0.96), rgba(236, 242, 252, 0.84));\n"
"  box-shadow: inset 0 1px 0 #ffffff,\n"
"              0 12px 28px rgba(15, 23, 42, 0.18);\n"
"  color: #111319;\n"
"  opacity: 1;\n"
"  transform: translateY(0px) scale(1);\n"
"  transition: opacity 200ms cubic-bezier(0.22, 1, 0.36, 1), transform 220ms cubic-bezier(0.22, 1, 0.36, 1), background-image 180ms ease, box-shadow 200ms ease;\n"
"}\n"
"button.fab-btn:hover {\n"
"  background-image: linear-gradient(160deg, #ffffff, #f4f7fd);\n"
"  box-shadow: inset 0 1px 0 #ffffff,\n"
"              0 16px 34px rgba(15, 23, 42, 0.24);\n"
"  transform: translateY(-2px) scale(1.05);\n"
"}\n"
"button.fab-btn:active {\n"
"  transform: translateY(0px) scale(0.94);\n"
"}\n"
"button.fab-btn.fab-hidden {\n"
"  opacity: 0;\n"
"  transform: translateY(14px) scale(0.76);\n"
"}\n"
"window.theme-dark button.fab-btn {\n"
"  border: 1px solid rgba(255, 255, 255, 0.28);\n"
"  background-image: linear-gradient(160deg, rgba(64, 74, 98, 0.92), rgba(40, 48, 66, 0.86));\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.22),\n"
"              0 14px 32px rgba(0, 0, 0, 0.50);\n"
"  color: #ffffff;\n"
"}\n"
"window.theme-dark button.fab-btn:hover {\n"
"  background-image: linear-gradient(160deg, rgba(80, 92, 122, 0.96), rgba(50, 58, 80, 0.92));\n"
"}\n"
".sas-card {\n"
"  border-radius: 20px;\n"
"  padding: 16px 20px;\n"
"  border: 1px solid rgba(245, 158, 11, 0.42);\n"
"  background-image: linear-gradient(145deg, rgba(255, 251, 235, 0.84), rgba(254, 243, 199, 0.64));\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.90),\n"
"              0 10px 26px rgba(245, 158, 11, 0.12);\n"
"}\n"
"window.theme-dark .sas-card {\n"
"  background-image: linear-gradient(145deg, rgba(69, 49, 17, 0.72), rgba(48, 34, 12, 0.62));\n"
"  border: 1px solid rgba(251, 191, 36, 0.36);\n"
"}\n"
".sas-code {\n"
"  font-family: monospace;\n"
"  font-size: 26px;\n"
"  font-weight: 800;\n"
"  letter-spacing: 5px;\n"
"  color: #2563eb;\n"
"}\n"
"window.theme-dark .sas-code {\n"
"  color: #7cb5ff;\n"
"}\n"
".construction-canvas {\n"
"  border-radius: 22px;\n"
"  border: 1px solid rgba(255, 255, 255, 0.86);\n"
"  background-color: transparent;\n"
"  background-image: linear-gradient(145deg, rgba(255, 255, 255, 0.68), rgba(242, 246, 254, 0.54));\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.90),\n"
"              0 10px 28px rgba(15, 23, 42, 0.06);\n"
"  transition: background-color 220ms ease, background-image 220ms ease, border-color 220ms ease, box-shadow 220ms ease, color 180ms ease;\n"
"}\n"
"window.theme-dark .construction-canvas {\n"
"  border: 1px solid rgba(255, 255, 255, 0.16);\n"
"  background-image: linear-gradient(145deg, rgba(38, 45, 64, 0.66), rgba(24, 29, 43, 0.52));\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.14),\n"
"              0 12px 32px rgba(0, 0, 0, 0.30);\n"
"}\n"
".construction-title {\n"
"  font-size: 24px;\n"
"  font-weight: 800;\n"
"}\n"
".modal-scrim {\n"
"  background-color: rgba(15, 23, 42, 0.24);\n"
"  border-radius: 24px;\n"
"  opacity: 0;\n"
"  transition: opacity 200ms cubic-bezier(0.22, 1, 0.36, 1);\n"
"}\n"
".modal-open .modal-scrim {\n"
"  opacity: 1;\n"
"}\n"
"window.theme-dark .modal-scrim {\n"
"  background-color: rgba(0, 0, 0, 0.48);\n"
"}\n"
".modal-card {\n"
"  border-radius: 26px;\n"
"  padding: 24px 26px;\n"
"  border: 1px solid rgba(255, 255, 255, 0.88);\n"
"  background-color: transparent;\n"
"  background-image: linear-gradient(155deg, rgba(255, 255, 255, 0.86), rgba(244, 247, 254, 0.72));\n"
"  box-shadow: inset 0 1px 0 #ffffff,\n"
"              0 28px 70px rgba(15, 23, 42, 0.24);\n"
"  opacity: 0;\n"
"  transform: translateY(16px) scale(0.95);\n"
"  transition: opacity 200ms cubic-bezier(0.22, 1, 0.36, 1), transform 230ms cubic-bezier(0.22, 1, 0.36, 1), background-image 220ms ease, border-color 220ms ease;\n"
"}\n"
".modal-open .modal-card {\n"
"  opacity: 1;\n"
"  transform: translateY(0px) scale(1);\n"
"}\n"
"window.theme-dark .modal-card {\n"
"  border: 1px solid rgba(255, 255, 255, 0.22);\n"
"  background-image: linear-gradient(155deg, rgba(34, 40, 56, 0.90), rgba(22, 26, 38, 0.82));\n"
"  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.18),\n"
"              0 28px 72px rgba(0, 0, 0, 0.65);\n"
"}\n"
".modal-title {\n"
"  font-size: 20px;\n"
"  font-weight: 800;\n"
"}\n"
"button.modal-close-btn {\n"
"  min-width: 40px;\n"
"  min-height: 40px;\n"
"  padding: 0;\n"
"  border-radius: 999px;\n"
"  border: 1px solid rgba(15, 23, 42, 0.08);\n"
"  background-image: none;\n"
"  background-color: #ffffff;\n"
"  box-shadow: 0 4px 14px rgba(15, 23, 42, 0.12);\n"
"  color: #111319;\n"
"  transform: scale(1) rotate(0deg);\n"
"  transition: background-color 170ms ease, transform 200ms cubic-bezier(0.22, 1, 0.36, 1), box-shadow 170ms ease;\n"
"}\n"
"button.modal-close-btn:hover {\n"
"  background-color: #f3f6fc;\n"
"  transform: scale(1.06) rotate(90deg);\n"
"}\n"
"button.modal-close-btn:active {\n"
"  transform: scale(0.93) rotate(90deg);\n"
"}\n"
"window.theme-dark button.modal-close-btn {\n"
"  border: 1px solid rgba(255, 255, 255, 0.22);\n"
"  background-color: rgba(255, 255, 255, 0.92);\n"
"  color: #111319;\n"
"}\n"
".seg-wrap {\n"
"  border-radius: 999px;\n"
"  padding: 3px;\n"
"  background-color: rgba(15, 23, 42, 0.08);\n"
"}\n"
"window.theme-dark .seg-wrap {\n"
"  background-color: rgba(255, 255, 255, 0.09);\n"
"}\n"
"button.seg-btn {\n"
"  border-radius: 999px;\n"
"  padding: 5px 14px;\n"
"  font-size: 12px;\n"
"  font-weight: 600;\n"
"  border: none;\n"
"  background-image: none;\n"
"  background-color: transparent;\n"
"  color: rgba(18, 21, 30, 0.72);\n"
"  transform: scale(1);\n"
"  transition: background-color 170ms cubic-bezier(0.22, 1, 0.36, 1), color 160ms ease, box-shadow 180ms ease, transform 140ms ease;\n"
"}\n"
"button.seg-btn:active {\n"
"  transform: scale(0.96);\n"
"}\n"
"window.theme-dark button.seg-btn {\n"
"  color: rgba(241, 245, 249, 0.72);\n"
"}\n"
"button.seg-btn.seg-active {\n"
"  background-color: #ffffff;\n"
"  color: #111319;\n"
"  box-shadow: 0 2px 8px rgba(15, 23, 42, 0.10);\n"
"}\n"
"window.theme-dark button.seg-btn.seg-active {\n"
"  background-color: rgba(255, 255, 255, 0.22);\n"
"  color: #ffffff;\n"
"}\n"
".setting-row {\n"
"  border-radius: 16px;\n"
"  padding: 12px 14px;\n"
"  border: 1px solid rgba(15, 23, 42, 0.08);\n"
"  background-color: rgba(255, 255, 255, 0.52);\n"
"  transition: background-color 220ms ease, background-image 220ms ease, border-color 220ms ease, box-shadow 220ms ease, color 180ms ease;\n"
"}\n"
"window.theme-dark .setting-row {\n"
"  border: 1px solid rgba(255, 255, 255, 0.10);\n"
"  background-color: rgba(255, 255, 255, 0.06);\n"
"}\n"
"entry.glass-entry {\n"
"  min-height: 36px;\n"
"  padding: 0 12px;\n"
"  border-radius: 12px;\n"
"  border: 1px solid rgba(15, 23, 42, 0.12);\n"
"  background-color: rgba(255, 255, 255, 0.78);\n"
"  background-image: none;\n"
"  color: #0f172a;\n"
"  font-size: 13px;\n"
"  font-weight: 600;\n"
"  transition: border-color 170ms ease, background-color 170ms ease, box-shadow 170ms ease;\n"
"}\n"
"entry.glass-entry:focus-within {\n"
"  border-color: rgba(37, 99, 235, 0.72);\n"
"  background-color: #ffffff;\n"
"  box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.16);\n"
"}\n"
"window.theme-dark entry.glass-entry {\n"
"  border: 1px solid rgba(255, 255, 255, 0.16);\n"
"  background-color: rgba(255, 255, 255, 0.09);\n"
"  color: #f1f5f9;\n"
"}\n"
"window.theme-dark entry.glass-entry:focus-within {\n"
"  border-color: rgba(96, 165, 250, 0.80);\n"
"  background-color: rgba(255, 255, 255, 0.14);\n"
"  box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.24);\n"
"}\n"
"progressbar trough {\n"
"  min-height: 7px;\n"
"  border-radius: 999px;\n"
"  background-color: rgba(15, 23, 42, 0.09);\n"
"}\n"
"window.theme-dark progressbar trough {\n"
"  background-color: rgba(255, 255, 255, 0.12);\n"
"}\n"
"progressbar progress {\n"
"  min-height: 7px;\n"
"  border-radius: 999px;\n"
"  background-image: linear-gradient(90deg, #3b82f6, #0ea5e9);\n"
"}\n"
"scrollbar slider {\n"
"  border-radius: 999px;\n"
"  background-color: rgba(100, 116, 139, 0.28);\n"
"  min-width: 6px;\n"
"}\n";

/* ---------------- Vector Cairo Icons & Brand Logo ---------------- */

static void draw_glyph(cairo_t *cr, WsIconKind kind, double size_px, double ox, double oy) {
    cairo_save(cr);
    double s = size_px / 24.0;
    cairo_translate(cr, ox, oy);
    cairo_scale(cr, s, s);
    cairo_set_line_width(cr, 2.05);
    cairo_set_line_cap(cr, CAIRO_LINE_CAP_ROUND);
    cairo_set_line_join(cr, CAIRO_LINE_JOIN_ROUND);

    switch (kind) {
    case ICON_DEVICES:
        cairo_rectangle(cr, 6.5, 3.0, 11.0, 18.0);
        cairo_stroke(cr);
        cairo_move_to(cr, 10.5, 18.0);
        cairo_line_to(cr, 13.5, 18.0);
        cairo_stroke(cr);
        break;
    case ICON_CLIPBOARD:
        cairo_rectangle(cr, 5.5, 5.0, 13.0, 16.0);
        cairo_stroke(cr);
        cairo_rectangle(cr, 9.0, 3.0, 6.0, 3.5);
        cairo_stroke(cr);
        cairo_move_to(cr, 8.5, 11.5);
        cairo_line_to(cr, 15.5, 11.5);
        cairo_move_to(cr, 8.5, 15.5);
        cairo_line_to(cr, 13.5, 15.5);
        cairo_stroke(cr);
        break;
    case ICON_TRANSFER:
        cairo_move_to(cr, 7.0, 8.0);
        cairo_line_to(cr, 17.0, 8.0);
        cairo_move_to(cr, 14.0, 5.0);
        cairo_line_to(cr, 17.0, 8.0);
        cairo_line_to(cr, 14.0, 11.0);
        cairo_move_to(cr, 17.0, 16.0);
        cairo_line_to(cr, 7.0, 16.0);
        cairo_move_to(cr, 10.0, 13.0);
        cairo_line_to(cr, 7.0, 16.0);
        cairo_line_to(cr, 10.0, 19.0);
        cairo_stroke(cr);
        break;
    case ICON_SCREEN:
        cairo_rectangle(cr, 3.5, 4.5, 17.0, 11.5);
        cairo_stroke(cr);
        cairo_move_to(cr, 8.0, 19.5);
        cairo_line_to(cr, 16.0, 19.5);
        cairo_move_to(cr, 12.0, 16.0);
        cairo_line_to(cr, 12.0, 19.5);
        cairo_stroke(cr);
        break;
    case ICON_NOTES:
        cairo_rectangle(cr, 5.5, 3.5, 13.0, 17.0);
        cairo_stroke(cr);
        cairo_move_to(cr, 8.5, 8.5);
        cairo_line_to(cr, 15.5, 8.5);
        cairo_move_to(cr, 8.5, 12.0);
        cairo_line_to(cr, 15.5, 12.0);
        cairo_move_to(cr, 8.5, 15.5);
        cairo_line_to(cr, 13.0, 15.5);
        cairo_stroke(cr);
        break;
    case ICON_SETTINGS:
        cairo_arc(cr, 12.0, 12.0, 3.2, 0.0, 2.0 * M_PI);
        cairo_stroke(cr);
        for (int i = 0; i < 6; ++i) {
            double ang = i * (M_PI / 3.0);
            cairo_move_to(cr, 12.0 + cos(ang) * 5.6, 12.0 + sin(ang) * 5.6);
            cairo_line_to(cr, 12.0 + cos(ang) * 8.2, 12.0 + sin(ang) * 8.2);
        }
        cairo_stroke(cr);
        break;
    case ICON_PLUS:
        cairo_set_line_width(cr, 2.5);
        cairo_move_to(cr, 12.0, 5.5);
        cairo_line_to(cr, 12.0, 18.5);
        cairo_move_to(cr, 5.5, 12.0);
        cairo_line_to(cr, 18.5, 12.0);
        cairo_stroke(cr);
        break;
    case ICON_CLOSE:
        cairo_set_line_width(cr, 2.6);
        cairo_move_to(cr, 6.5, 6.5);
        cairo_line_to(cr, 17.5, 17.5);
        cairo_move_to(cr, 17.5, 6.5);
        cairo_line_to(cr, 6.5, 17.5);
        cairo_stroke(cr);
        break;
    case ICON_HIDE:
        cairo_set_line_width(cr, 2.2);
        cairo_move_to(cr, 7.0, 12.0);
        cairo_line_to(cr, 17.0, 12.0);
        cairo_stroke(cr);
        break;
    case ICON_SEND:
        cairo_move_to(cr, 4.0, 12.0);
        cairo_line_to(cr, 20.0, 5.0);
        cairo_line_to(cr, 14.5, 19.5);
        cairo_line_to(cr, 11.5, 13.0);
        cairo_close_path(cr);
        cairo_stroke(cr);
        break;
    case ICON_FOLDER:
        cairo_move_to(cr, 3.5, 6.5);
        cairo_line_to(cr, 9.5, 6.5);
        cairo_line_to(cr, 11.5, 8.8);
        cairo_line_to(cr, 20.5, 8.8);
        cairo_line_to(cr, 20.5, 18.5);
        cairo_line_to(cr, 3.5, 18.5);
        cairo_close_path(cr);
        cairo_stroke(cr);
        break;
    }
    cairo_restore(cr);
}

static void draw_icon_cb(GtkDrawingArea *area, cairo_t *cr, int w, int h, gpointer data) {
    WsIconKind kind = (WsIconKind)(intptr_t)data;
    GdkRGBA color;
    gtk_widget_get_color(GTK_WIDGET(area), &color);
    double d = (double)(w < h ? w : h);
    cairo_set_source_rgba(cr, color.red, color.green, color.blue, color.alpha);
    draw_glyph(cr, kind, d, (w - d) * 0.5, (h - d) * 0.5);
}

static GtkWidget *make_icon(WsIconKind kind, int px) {
    GtkWidget *area = gtk_drawing_area_new();
    gtk_drawing_area_set_content_width(GTK_DRAWING_AREA(area), px);
    gtk_drawing_area_set_content_height(GTK_DRAWING_AREA(area), px);
    gtk_widget_set_hexpand(area, FALSE);
    gtk_widget_set_vexpand(area, FALSE);
    gtk_widget_set_halign(area, GTK_ALIGN_CENTER);
    gtk_widget_set_valign(area, GTK_ALIGN_CENTER);
    gtk_drawing_area_set_draw_func(GTK_DRAWING_AREA(area), draw_icon_cb, (gpointer)(intptr_t)kind, NULL);
    return area;
}

static void draw_nav_badge_cb(GtkDrawingArea *area, cairo_t *cr, int w, int h, gpointer data) {
    (void)area;
    WsIconKind kind = (WsIconKind)(intptr_t)data;
    double cx = w * 0.5;
    double cy = h * 0.5;
    double r = (w < h ? w : h) * 0.5 - 0.5;

    cairo_pattern_t *grad = cairo_pattern_create_linear(0.0, 0.0, 0.0, (double)h);
    cairo_pattern_add_color_stop_rgba(grad, 0.0, 0.14, 0.58, 1.0, 1.0);
    cairo_pattern_add_color_stop_rgba(grad, 1.0, 0.04, 0.48, 0.96, 1.0);
    cairo_arc(cr, cx, cy, r, 0.0, 2.0 * M_PI);
    cairo_set_source(cr, grad);
    cairo_fill(cr);
    cairo_pattern_destroy(grad);

    cairo_set_source_rgba(cr, 1.0, 1.0, 1.0, 0.35);
    cairo_set_line_width(cr, 1.0);
    cairo_arc(cr, cx, cy, r - 0.5, M_PI * 1.12, M_PI * 1.88);
    cairo_stroke(cr);

    cairo_set_source_rgba(cr, 1.0, 1.0, 1.0, 0.98);
    double gsize = 15.0;
    draw_glyph(cr, kind, gsize, cx - gsize * 0.5, cy - gsize * 0.5);
}

static GtkWidget *make_nav_badge_icon(WsIconKind kind, int px) {
    GtkWidget *area = gtk_drawing_area_new();
    gtk_drawing_area_set_content_width(GTK_DRAWING_AREA(area), px);
    gtk_drawing_area_set_content_height(GTK_DRAWING_AREA(area), px);
    gtk_widget_set_hexpand(area, FALSE);
    gtk_widget_set_vexpand(area, FALSE);
    gtk_widget_set_halign(area, GTK_ALIGN_START);
    gtk_widget_set_valign(area, GTK_ALIGN_CENTER);
    gtk_drawing_area_set_draw_func(GTK_DRAWING_AREA(area), draw_nav_badge_cb, (gpointer)(intptr_t)kind, NULL);
    return area;
}

static void draw_logo_cb(GtkDrawingArea *area, cairo_t *cr, int w, int h, gpointer data) {
    (void)area;
    WsUiState *st = (WsUiState *)data;
    double cx = w * 0.5;
    double cy = h * 0.5;
    double r = (w < h ? w : h) * 0.5 - 0.5;

    cairo_save(cr);
    cairo_arc(cr, cx, cy, r, 0.0, 2.0 * M_PI);
    cairo_clip(cr);

    if (st->logo_surface) {
        double pw = (double)cairo_image_surface_get_width(st->logo_surface);
        double ph = (double)cairo_image_surface_get_height(st->logo_surface);
        cairo_scale(cr, (double)w / (pw > 0.0 ? pw : 1.0), (double)h / (ph > 0.0 ? ph : 1.0));
        cairo_set_source_surface(cr, st->logo_surface, 0.0, 0.0);
        cairo_pattern_set_filter(cairo_get_source(cr), CAIRO_FILTER_BILINEAR);
        cairo_paint(cr);
    } else {
        cairo_set_source_rgb(cr, 0.90, 0.08, 0.12);
        cairo_paint(cr);
    }
    cairo_restore(cr);

    cairo_set_source_rgba(cr, 1.0, 1.0, 1.0, 0.45);
    cairo_set_line_width(cr, 1.2);
    cairo_arc(cr, cx, cy, r - 0.6, M_PI * 1.1, M_PI * 1.9);
    cairo_stroke(cr);
}

static GtkWidget *make_logo_widget(WsUiState *st) {
    GtkWidget *area = gtk_drawing_area_new();
    gtk_drawing_area_set_content_width(GTK_DRAWING_AREA(area), 44);
    gtk_drawing_area_set_content_height(GTK_DRAWING_AREA(area), 44);
    gtk_widget_set_hexpand(area, FALSE);
    gtk_widget_set_vexpand(area, FALSE);
    gtk_widget_set_halign(area, GTK_ALIGN_START);
    gtk_widget_set_valign(area, GTK_ALIGN_CENTER);
    gtk_drawing_area_set_draw_func(GTK_DRAWING_AREA(area), draw_logo_cb, st, NULL);
    return area;
}

/* ---------------- Theme Management ---------------- */

static void apply_theme(WsUiState *st) {
    int want_dark = 0;
    if (st->theme_mode == THEME_DARK) {
        want_dark = 1;
    } else if (st->theme_mode == THEME_SYSTEM) {
        gboolean pref = FALSE;
        GtkSettings *gs = gtk_settings_get_default();
        if (gs) {
            g_object_get(gs, "gtk-application-prefer-dark-theme", &pref, NULL);
        }
        want_dark = pref ? 1 : 0;
    }

    st->is_dark = want_dark;
    if (want_dark) {
        gtk_widget_add_css_class(st->window, "theme-dark");
    } else {
        gtk_widget_remove_css_class(st->window, "theme-dark");
    }
    for (int i = 0; i < 3; ++i) {
        if (!st->theme_btns[i]) continue;
        if (i == (int)st->theme_mode) {
            gtk_widget_add_css_class(st->theme_btns[i], "seg-active");
        } else {
            gtk_widget_remove_css_class(st->theme_btns[i], "seg-active");
        }
    }
}

static void on_theme_btn_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    IdActionData *d = (IdActionData *)data;
    if (strcmp(d->id, "light") == 0) d->st->theme_mode = THEME_LIGHT;
    else if (strcmp(d->id, "dark") == 0) d->st->theme_mode = THEME_DARK;
    else d->st->theme_mode = THEME_SYSTEM;
    apply_theme(d->st);
}

/* ---------------- Navigation & Modal ---------------- */

static gboolean modal_open_kick_cb(gpointer data) {
    WsUiState *st = (WsUiState *)data;
    if (!st) return G_SOURCE_REMOVE;
    st->modal_open_source_id = 0;
    if (st->modal_kind != MODAL_NONE && st->modal_scrim_box) {
        gtk_widget_add_css_class(st->modal_scrim_box, "modal-open");
    }
    return G_SOURCE_REMOVE;
}

static gboolean modal_hide_finish_cb(gpointer data) {
    WsUiState *st = (WsUiState *)data;
    if (!st) return G_SOURCE_REMOVE;
    st->modal_close_source_id = 0;
    if (st->modal_kind == MODAL_NONE && st->modal_scrim_box) {
        gtk_widget_set_visible(st->modal_scrim_box, FALSE);
    }
    return G_SOURCE_REMOVE;
}

static gboolean win_present_finish_cb(gpointer data) {
    WsUiState *st = (WsUiState *)data;
    if (!st) return G_SOURCE_REMOVE;
    st->win_anim_source_id = 0;
    if (st->stage_overlay) {
        gtk_widget_remove_css_class(st->stage_overlay, "win-presenting");
    }
    return G_SOURCE_REMOVE;
}

static void set_active_page(WsUiState *st, WsPage page) {
    if (st->nav_initialized && page == st->active_page) {
        return;
    }
    int going_down = ((int)page >= (int)st->active_page);
    st->active_page = page;
    const char *names[5] = {"devices", "clipboard", "transfers", "screen", "notes"};
    for (int i = 0; i < 5; ++i) {
        if (!st->pages[i]) continue;
        gtk_widget_remove_css_class(st->pages[i], "page-enter-down");
        gtk_widget_remove_css_class(st->pages[i], "page-enter-up");
    }
    if (st->nav_initialized && st->pages[page]) {
        gtk_widget_add_css_class(st->pages[page], going_down ? "page-enter-down" : "page-enter-up");
    }
    gtk_stack_set_visible_child_name(GTK_STACK(st->content_stack), names[page]);
    for (int i = 0; i < 5; ++i) {
        if (!st->nav_buttons[i]) continue;
        if (i == (int)page) {
            gtk_widget_add_css_class(st->nav_buttons[i], "nav-active");
        } else {
            gtk_widget_remove_css_class(st->nav_buttons[i], "nav-active");
        }
    }
    int show_fab = (page == PAGE_DEVICES);
    gtk_widget_set_visible(st->fab_btn, TRUE);
    gtk_widget_set_can_target(st->fab_btn, show_fab);
    if (show_fab) {
        gtk_widget_remove_css_class(st->fab_btn, "fab-hidden");
    } else {
        gtk_widget_add_css_class(st->fab_btn, "fab-hidden");
    }
    st->nav_initialized = 1;
}

static void set_modal(WsUiState *st, WsModalKind kind) {
    if (st->modal_open_source_id) {
        g_source_remove(st->modal_open_source_id);
        st->modal_open_source_id = 0;
    }
    if (st->modal_close_source_id) {
        g_source_remove(st->modal_close_source_id);
        st->modal_close_source_id = 0;
    }

    st->modal_kind = kind;
    if (kind == MODAL_NONE) {
        gtk_widget_set_can_target(st->modal_scrim_box, FALSE);
        gtk_widget_remove_css_class(st->stage_overlay, "stage-blurred");
        gtk_widget_remove_css_class(st->modal_scrim_box, "modal-open");
        if (gtk_widget_get_visible(st->modal_scrim_box)) {
            st->modal_close_source_id = g_timeout_add(230, modal_hide_finish_cb, st);
        }
        return;
    }

    if (kind == MODAL_SETTINGS) {
        gtk_label_set_text(GTK_LABEL(st->modal_title_label), "设置");
        if (st->user_name_entry) {
            gtk_editable_set_text(GTK_EDITABLE(st->user_name_entry), st->display_user ? st->display_user : "");
        }
        if (st->device_name_entry) {
            gtk_editable_set_text(GTK_EDITABLE(st->device_name_entry), st->current_device_name ? st->current_device_name : "");
        }
        gtk_stack_set_visible_child_name(GTK_STACK(st->modal_body_stack), "settings");
    } else {
        gtk_label_set_text(GTK_LABEL(st->modal_title_label), "同一局域网发现设备");
        gtk_stack_set_visible_child_name(GTK_STACK(st->modal_body_stack), "discover");
    }
    gtk_widget_set_can_target(st->modal_scrim_box, TRUE);
    gtk_widget_add_css_class(st->stage_overlay, "stage-blurred");
    if (!gtk_widget_get_visible(st->modal_scrim_box)) {
        gtk_widget_remove_css_class(st->modal_scrim_box, "modal-open");
        gtk_widget_set_visible(st->modal_scrim_box, TRUE);
        st->modal_open_source_id = g_timeout_add(16, modal_open_kick_cb, st);
    } else {
        gtk_widget_add_css_class(st->modal_scrim_box, "modal-open");
    }
}

static void on_nav_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    IdActionData *d = (IdActionData *)data;
    int idx = atoi(d->id);
    if (idx >= 0 && idx < 5) {
        set_active_page(d->st, (WsPage)idx);
    }
}

static void on_open_settings_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    set_modal((WsUiState *)data, MODAL_SETTINGS);
}

static void on_open_discover_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    set_modal((WsUiState *)data, MODAL_DISCOVER);
}

static void on_close_modal_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    set_modal((WsUiState *)data, MODAL_NONE);
}

static void on_scrim_pressed(GtkGestureClick *g, int n_press, double x, double y, gpointer data) {
    (void)g; (void)n_press; (void)x; (void)y;
    set_modal((WsUiState *)data, MODAL_NONE);
}

static gboolean on_window_key_pressed(GtkEventControllerKey *ctrl, guint keyval,
                                      guint keycode, GdkModifierType state, gpointer data) {
    (void)ctrl; (void)keycode; (void)state;
    WsUiState *st = (WsUiState *)data;
    if (keyval == GDK_KEY_Escape && st->modal_kind != MODAL_NONE) {
        set_modal(st, MODAL_NONE);
        return TRUE;
    }
    return FALSE;
}

static void on_hide_window_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    WsUiState *st = (WsUiState *)data;
    if (st->window) {
        gtk_widget_set_visible(st->window, FALSE);
    }
}

/* ---------------- Action Callbacks ---------------- */

static void on_pair_start_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    IdActionData *d = (IdActionData *)data;
    set_modal(d->st, MODAL_NONE);
    if (d->st->cb.on_pair_start) {
        d->st->cb.on_pair_start(d->id, d->st->user_data);
    }
}

static void on_pair_approve_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    IdActionData *d = (IdActionData *)data;
    if (d->st->cb.on_pair_approve) {
        d->st->cb.on_pair_approve(d->id, d->st->user_data);
    }
}

static void on_pair_reject_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    IdActionData *d = (IdActionData *)data;
    if (d->st->cb.on_pair_reject) {
        d->st->cb.on_pair_reject(d->id, d->st->user_data);
    }
}

static void on_unpair_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    IdActionData *d = (IdActionData *)data;
    if (d->st->cb.on_unpair) {
        d->st->cb.on_unpair(d->id, d->st->user_data);
    }
}

static void on_open_downloads_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    WsUiState *st = (WsUiState *)data;
    if (st->cb.on_open_downloads) {
        st->cb.on_open_downloads(st->user_data);
    }
}

static void on_quit_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    WsUiState *st = (WsUiState *)data;
    if (st->cb.on_quit_clicked) {
        st->cb.on_quit_clicked(st->user_data);
    }
}

static int entry_has_active_focus(WsUiState *st, GtkWidget *entry) {
    if (!st || !st->window || !entry) return 0;
    GtkWidget *f = gtk_window_get_focus(GTK_WINDOW(st->window));
    return f && (f == entry || gtk_widget_is_ancestor(f, entry));
}

static void save_user_name_from_entry(WsUiState *st) {
    if (!st || !st->user_name_entry) return;
    const char *txt = gtk_editable_get_text(GTK_EDITABLE(st->user_name_entry));
    if (st->window) {
        gtk_window_set_focus(GTK_WINDOW(st->window), NULL);
    }
    if (st->cb.on_set_user_name) {
        st->cb.on_set_user_name(txt ? txt : "", st->user_data);
    }
}

static void save_device_name_from_entry(WsUiState *st) {
    if (!st || !st->device_name_entry) return;
    const char *txt = gtk_editable_get_text(GTK_EDITABLE(st->device_name_entry));
    if (st->window) {
        gtk_window_set_focus(GTK_WINDOW(st->window), NULL);
    }
    if (st->cb.on_set_device_name) {
        st->cb.on_set_device_name(txt ? txt : "", st->user_data);
    }
}

static void on_save_user_name_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    save_user_name_from_entry((WsUiState *)data);
}

static void on_user_name_entry_activate(GtkEntry *entry, gpointer data) {
    (void)entry;
    save_user_name_from_entry((WsUiState *)data);
}

static void on_save_device_name_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    save_device_name_from_entry((WsUiState *)data);
}

static void on_device_name_entry_activate(GtkEntry *entry, gpointer data) {
    (void)entry;
    save_device_name_from_entry((WsUiState *)data);
}

static gboolean on_sync_switch_state_set(GtkSwitch *sw, gboolean state, gpointer data) {
    (void)sw;
    WsUiState *st = (WsUiState *)data;
    if (st->updating_sync_switch) {
        return FALSE;
    }
    int paused = state ? 0 : 1;
    if (st->cb.on_set_sync_paused) {
        st->cb.on_set_sync_paused(paused, st->user_data);
    }
    return FALSE;
}

static void on_files_picked(GObject *src, GAsyncResult *res, gpointer data) {
    IdActionData *d = (IdActionData *)data;
    GError *err = NULL;
    GListModel *files = gtk_file_dialog_open_multiple_finish(GTK_FILE_DIALOG(src), res, &err);
    if (err) {
        g_error_free(err);
    }
    if (files) {
        guint n = g_list_model_get_n_items(files);
        if (n > 0 && d->st->cb.on_send_files) {
            const char **paths = g_new0(const char *, n);
            char **owned = g_new0(char *, n);
            guint count = 0;
            for (guint i = 0; i < n; ++i) {
                GFile *f = G_FILE(g_list_model_get_item(files, i));
                if (f) {
                    char *p = g_file_get_path(f);
                    if (p) {
                        owned[count] = p;
                        paths[count] = p;
                        count++;
                    }
                    g_object_unref(f);
                }
            }
            if (count > 0) {
                d->st->cb.on_send_files(d->id, paths, (int)count, d->st->user_data);
            }
            for (guint i = 0; i < count; ++i) {
                g_free(owned[i]);
            }
            g_free(owned);
            g_free(paths);
        }
        g_object_unref(files);
    }
    g_object_unref(src);
    g_free(d->id);
    g_free(d);
}

static void on_pick_files_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    IdActionData *d = (IdActionData *)data;
    IdActionData *req = id_action_new(d->st, d->id);
    GtkFileDialog *dlg = gtk_file_dialog_new();
    gtk_file_dialog_set_title(dlg, "选择要发送的文件");
    gtk_file_dialog_open_multiple(dlg, GTK_WINDOW(d->st->window), NULL, on_files_picked, req);
}

/* ---------------- Time Greeting & RTT Helpers ---------------- */

static const char *time_greeting_prefix(void) {
    time_t now = time(NULL);
    struct tm local_tm;
    localtime_r(&now, &local_tm);
    int h = local_tm.tm_hour;
    if (h < 6) return "凌晨好";
    if (h < 9) return "早上好";
    if (h < 12) return "上午好";
    if (h < 14) return "中午好";
    if (h < 18) return "下午好";
    return "晚上好";
}

static void refresh_greeting(WsUiState *st) {
    char buf[256];
    snprintf(buf, sizeof(buf), "%s，%s !", time_greeting_prefix(),
             st->display_user ? st->display_user : "User");
    gtk_label_set_text(GTK_LABEL(st->greeting_label), buf);
}

static void apply_rtt_to_labels(GtkWidget *hero_lbl, GtkWidget *cap_lbl, int connected, int64_t rtt_ms) {
    if (!hero_lbl || !cap_lbl) return;
    char hero_txt[48];
    const char *hero_class = "metric-muted";
    const char *cap_txt = "等待设备上线";
    if (connected) {
        if (rtt_ms >= 0) {
            snprintf(hero_txt, sizeof(hero_txt), "%lldms", (long long)rtt_ms);
            cap_txt = "局域网心跳延迟";
            if (rtt_ms < 25) hero_class = "metric-fast";
            else if (rtt_ms < 90) hero_class = "metric-warm";
            else hero_class = "metric-slow";
        } else {
            snprintf(hero_txt, sizeof(hero_txt), "已连");
            cap_txt = "mTLS 通道就绪";
            hero_class = "metric-fast";
        }
    } else {
        snprintf(hero_txt, sizeof(hero_txt), "离线");
    }

    gtk_widget_remove_css_class(hero_lbl, "metric-fast");
    gtk_widget_remove_css_class(hero_lbl, "metric-warm");
    gtk_widget_remove_css_class(hero_lbl, "metric-slow");
    gtk_widget_remove_css_class(hero_lbl, "metric-muted");
    gtk_widget_add_css_class(hero_lbl, hero_class);
    gtk_label_set_text(GTK_LABEL(hero_lbl), hero_txt);
    gtk_label_set_text(GTK_LABEL(cap_lbl), cap_txt);
}

/* ---------------- Widget Builders ---------------- */

static GtkWidget *make_nav_button(WsUiState *st, WsIconKind icon_kind, const char *text,
                                  int page_idx, int is_settings) {
    GtkWidget *btn = gtk_button_new();
    gtk_button_set_has_frame(GTK_BUTTON(btn), FALSE);
    gtk_widget_add_css_class(btn, "nav-btn");
    gtk_widget_set_size_request(btn, -1, is_settings ? 44 : 46);
    gtk_widget_set_hexpand(btn, TRUE);
    gtk_widget_set_vexpand(btn, FALSE);
    gtk_widget_set_valign(btn, GTK_ALIGN_START);
    gtk_widget_set_cursor_from_name(btn, "pointer");

    GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 12);
    gtk_widget_set_hexpand(row, TRUE);
    gtk_widget_set_vexpand(row, FALSE);
    gtk_widget_set_valign(row, GTK_ALIGN_CENTER);

    GtkWidget *badge = make_nav_badge_icon(icon_kind, 28);

    GtkWidget *lbl = gtk_label_new(text);
    gtk_label_set_xalign(GTK_LABEL(lbl), 0.0f);
    gtk_widget_set_hexpand(lbl, TRUE);
    gtk_widget_set_vexpand(lbl, FALSE);
    gtk_widget_set_valign(lbl, GTK_ALIGN_CENTER);

    gtk_box_append(GTK_BOX(row), badge);
    gtk_box_append(GTK_BOX(row), lbl);
    gtk_button_set_child(GTK_BUTTON(btn), row);

    if (is_settings) {
        g_signal_connect(btn, "clicked", G_CALLBACK(on_open_settings_clicked), st);
    } else {
        char idx_str[8];
        snprintf(idx_str, sizeof(idx_str), "%d", page_idx);
        g_signal_connect_data(btn, "clicked", G_CALLBACK(on_nav_clicked),
                              id_action_new(st, idx_str), id_action_free, 0);
    }
    return btn;
}

static GtkWidget *build_sidebar(WsUiState *st) {
    GtkWidget *sidebar = gtk_box_new(GTK_ORIENTATION_VERTICAL, 10);
    gtk_widget_add_css_class(sidebar, "glass-sidebar");
    gtk_widget_set_size_request(sidebar, 248, -1);
    gtk_widget_set_hexpand(sidebar, FALSE);
    gtk_widget_set_vexpand(sidebar, TRUE);

    /* Brand header wrapped in GtkWindowHandle so user can drag frameless window cleanly */
    GtkWidget *handle = gtk_window_handle_new();
    gtk_widget_set_hexpand(handle, TRUE);
    gtk_widget_set_vexpand(handle, FALSE);
    gtk_widget_set_valign(handle, GTK_ALIGN_START);

    GtkWidget *brand_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 12);
    gtk_widget_set_margin_bottom(brand_row, 14);
    gtk_widget_set_hexpand(brand_row, TRUE);
    gtk_widget_set_vexpand(brand_row, FALSE);

    GtkWidget *logo = make_logo_widget(st);
    gtk_box_append(GTK_BOX(brand_row), logo);

    GtkWidget *title_col = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
    gtk_widget_set_hexpand(title_col, TRUE);
    gtk_widget_set_vexpand(title_col, FALSE);
    gtk_widget_set_valign(title_col, GTK_ALIGN_CENTER);
    GtkWidget *b_title = gtk_label_new("寰宇工作台");
    gtk_widget_add_css_class(b_title, "brand-title");
    gtk_label_set_xalign(GTK_LABEL(b_title), 0.0f);
    GtkWidget *b_sub = gtk_label_new(WS_VERSION_DISPLAY " · 海纳百川，有容乃大");
    gtk_widget_add_css_class(b_sub, "brand-sub");
    gtk_label_set_xalign(GTK_LABEL(b_sub), 0.0f);
    gtk_label_set_ellipsize(GTK_LABEL(b_sub), PANGO_ELLIPSIZE_END);
    gtk_box_append(GTK_BOX(title_col), b_title);
    gtk_box_append(GTK_BOX(title_col), b_sub);
    gtk_box_append(GTK_BOX(brand_row), title_col);

    gtk_window_handle_set_child(GTK_WINDOW_HANDLE(handle), brand_row);
    gtk_box_append(GTK_BOX(sidebar), handle);

    const char *labels[5] = {"设备状态", "剪贴板共享", "文件传输", "屏幕共享", "OPPO 笔记"};
    const WsIconKind icons[5] = {ICON_DEVICES, ICON_CLIPBOARD, ICON_TRANSFER, ICON_SCREEN, ICON_NOTES};
    for (int i = 0; i < 5; ++i) {
        st->nav_buttons[i] = make_nav_button(st, icons[i], labels[i], i, 0);
        gtk_box_append(GTK_BOX(sidebar), st->nav_buttons[i]);
    }

    GtkWidget *spacer = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    gtk_widget_set_vexpand(spacer, TRUE);
    gtk_box_append(GTK_BOX(sidebar), spacer);

    st->settings_nav_btn = make_nav_button(st, ICON_SETTINGS, "设置", -1, 1);
    gtk_box_append(GTK_BOX(sidebar), st->settings_nav_btn);
    return sidebar;
}

static GtkWidget *build_construction_page(const char *page_heading, const char *center_title, const char *desc) {
    GtkWidget *page = gtk_box_new(GTK_ORIENTATION_VERTICAL, 18);

    GtkWidget *heading = gtk_label_new(page_heading);
    gtk_widget_add_css_class(heading, "hero-title");
    gtk_label_set_xalign(GTK_LABEL(heading), 0.0f);
    gtk_widget_set_vexpand(heading, FALSE);
    gtk_box_append(GTK_BOX(page), heading);

    GtkWidget *canvas = gtk_box_new(GTK_ORIENTATION_VERTICAL, 12);
    gtk_widget_add_css_class(canvas, "construction-canvas");
    gtk_widget_set_hexpand(canvas, TRUE);
    gtk_widget_set_vexpand(canvas, TRUE);

    GtkWidget *inner = gtk_box_new(GTK_ORIENTATION_VERTICAL, 10);
    gtk_widget_set_halign(inner, GTK_ALIGN_CENTER);
    gtk_widget_set_valign(inner, GTK_ALIGN_CENTER);
    gtk_widget_set_vexpand(inner, TRUE);

    GtkWidget *c_title = gtk_label_new(center_title);
    gtk_widget_add_css_class(c_title, "construction-title");

    GtkWidget *badge = gtk_label_new("施工中");
    gtk_widget_add_css_class(badge, "status-pill");
    gtk_widget_add_css_class(badge, "pill-amber");
    gtk_widget_set_halign(badge, GTK_ALIGN_CENTER);

    GtkWidget *c_desc = gtk_label_new(desc);
    gtk_widget_add_css_class(c_desc, "hero-sub");

    gtk_box_append(GTK_BOX(inner), c_title);
    gtk_box_append(GTK_BOX(inner), badge);
    gtk_box_append(GTK_BOX(inner), c_desc);
    gtk_box_append(GTK_BOX(canvas), inner);
    gtk_box_append(GTK_BOX(page), canvas);
    return page;
}

static GtkWidget *build_devices_page(WsUiState *st) {
    GtkWidget *page = gtk_box_new(GTK_ORIENTATION_VERTICAL, 16);

    st->greeting_label = gtk_label_new("");
    gtk_widget_add_css_class(st->greeting_label, "hero-title");
    gtk_label_set_xalign(GTK_LABEL(st->greeting_label), 0.0f);
    gtk_widget_set_vexpand(st->greeting_label, FALSE);
    refresh_greeting(st);
    gtk_box_append(GTK_BOX(page), st->greeting_label);

    st->sas_revealer = gtk_revealer_new();
    gtk_revealer_set_transition_type(GTK_REVEALER(st->sas_revealer), GTK_REVEALER_TRANSITION_TYPE_SLIDE_DOWN);
    gtk_revealer_set_transition_duration(GTK_REVEALER(st->sas_revealer), 240);
    gtk_revealer_set_reveal_child(GTK_REVEALER(st->sas_revealer), FALSE);
    gtk_widget_set_vexpand(st->sas_revealer, FALSE);

    st->sas_container = gtk_box_new(GTK_ORIENTATION_VERTICAL, 10);
    gtk_widget_set_visible(st->sas_container, TRUE);
    gtk_widget_set_vexpand(st->sas_container, FALSE);
    gtk_revealer_set_child(GTK_REVEALER(st->sas_revealer), st->sas_container);
    gtk_box_append(GTK_BOX(page), st->sas_revealer);

    GtkWidget *hdr_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
    gtk_widget_set_vexpand(hdr_row, FALSE);
    GtkWidget *sec = gtk_label_new("已连接的设备");
    gtk_widget_add_css_class(sec, "section-title");
    gtk_label_set_xalign(GTK_LABEL(sec), 0.0f);
    gtk_widget_set_hexpand(sec, TRUE);

    st->lan_status_badge = gtk_label_new("局域网通道运行中");
    gtk_widget_add_css_class(st->lan_status_badge, "status-pill");
    gtk_widget_add_css_class(st->lan_status_badge, "pill-info");
    gtk_widget_set_valign(st->lan_status_badge, GTK_ALIGN_CENTER);

    gtk_box_append(GTK_BOX(hdr_row), sec);
    gtk_box_append(GTK_BOX(hdr_row), st->lan_status_badge);
    gtk_box_append(GTK_BOX(page), hdr_row);

    GtkWidget *scroll = gtk_scrolled_window_new();
    gtk_scrolled_window_set_policy(GTK_SCROLLED_WINDOW(scroll), GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
    gtk_widget_set_vexpand(scroll, TRUE);

    GtkWidget *grid_wrap = gtk_box_new(GTK_ORIENTATION_VERTICAL, 16);
    gtk_widget_set_valign(grid_wrap, GTK_ALIGN_START);

    st->paired_flow = gtk_flow_box_new();
    gtk_flow_box_set_selection_mode(GTK_FLOW_BOX(st->paired_flow), GTK_SELECTION_NONE);
    gtk_flow_box_set_min_children_per_line(GTK_FLOW_BOX(st->paired_flow), 1);
    gtk_flow_box_set_max_children_per_line(GTK_FLOW_BOX(st->paired_flow), 3);
    gtk_flow_box_set_column_spacing(GTK_FLOW_BOX(st->paired_flow), 20);
    gtk_flow_box_set_row_spacing(GTK_FLOW_BOX(st->paired_flow), 20);
    gtk_flow_box_set_homogeneous(GTK_FLOW_BOX(st->paired_flow), FALSE);
    gtk_widget_set_halign(st->paired_flow, GTK_ALIGN_START);
    gtk_widget_set_valign(st->paired_flow, GTK_ALIGN_START);
    gtk_widget_set_hexpand(st->paired_flow, FALSE);
    gtk_widget_set_vexpand(st->paired_flow, FALSE);

    st->paired_empty_card = gtk_box_new(GTK_ORIENTATION_VERTICAL, 10);
    gtk_widget_add_css_class(st->paired_empty_card, "glass-card");
    gtk_widget_set_size_request(st->paired_empty_card, 460, -1);
    gtk_widget_set_halign(st->paired_empty_card, GTK_ALIGN_START);
    gtk_widget_set_valign(st->paired_empty_card, GTK_ALIGN_START);

    GtkWidget *empty_t = gtk_label_new("暂无已配对手机");
    gtk_widget_add_css_class(empty_t, "card-title");
    gtk_label_set_xalign(GTK_LABEL(empty_t), 0.0f);
    GtkWidget *empty_s = gtk_label_new("点击右下角 + 按钮发现同一局域网下的手机并发起 6 位验证码配对");
    gtk_widget_add_css_class(empty_s, "hero-sub");
    gtk_label_set_xalign(GTK_LABEL(empty_s), 0.0f);
    GtkWidget *disc_btn = gtk_button_new_with_label("发现局域网设备");
    gtk_button_set_has_frame(GTK_BUTTON(disc_btn), FALSE);
    gtk_widget_add_css_class(disc_btn, "glass-btn-primary");
    gtk_widget_set_halign(disc_btn, GTK_ALIGN_START);
    gtk_widget_set_cursor_from_name(disc_btn, "pointer");
    g_signal_connect(disc_btn, "clicked", G_CALLBACK(on_open_discover_clicked), st);
    gtk_box_append(GTK_BOX(st->paired_empty_card), empty_t);
    gtk_box_append(GTK_BOX(st->paired_empty_card), empty_s);
    gtk_box_append(GTK_BOX(st->paired_empty_card), disc_btn);

    gtk_box_append(GTK_BOX(grid_wrap), st->paired_flow);
    gtk_box_append(GTK_BOX(grid_wrap), st->paired_empty_card);
    gtk_scrolled_window_set_child(GTK_SCROLLED_WINDOW(scroll), grid_wrap);
    gtk_box_append(GTK_BOX(page), scroll);
    return page;
}

static GtkWidget *build_clipboard_page(WsUiState *st) {
    GtkWidget *page = gtk_box_new(GTK_ORIENTATION_VERTICAL, 18);

    GtkWidget *hdr = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
    gtk_widget_set_vexpand(hdr, FALSE);
    GtkWidget *t = gtk_label_new("剪贴板共享");
    gtk_widget_add_css_class(t, "hero-title");
    gtk_label_set_xalign(GTK_LABEL(t), 0.0f);
    GtkWidget *s = gtk_label_new("Android-master 无损双向剪贴板同步（文本 / PNG / JPEG / WebP）");
    gtk_widget_add_css_class(s, "hero-sub");
    gtk_label_set_xalign(GTK_LABEL(s), 0.0f);
    gtk_box_append(GTK_BOX(hdr), t);
    gtk_box_append(GTK_BOX(hdr), s);
    gtk_box_append(GTK_BOX(page), hdr);

    GtkWidget *ctrl_card = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 14);
    gtk_widget_add_css_class(ctrl_card, "glass-card");
    gtk_widget_set_vexpand(ctrl_card, FALSE);
    gtk_widget_set_valign(ctrl_card, GTK_ALIGN_START);

    GtkWidget *ctrl_col = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
    gtk_widget_set_hexpand(ctrl_col, TRUE);
    gtk_widget_set_vexpand(ctrl_col, FALSE);
    GtkWidget *c_t = gtk_label_new("启用剪贴板实时同步");
    gtk_widget_add_css_class(c_t, "card-title");
    gtk_label_set_xalign(GTK_LABEL(c_t), 0.0f);
    GtkWidget *c_s = gtk_label_new("暂停期间仍保持与手机端 mTLS 心跳连接，文件传输不受影响");
    gtk_widget_add_css_class(c_s, "hero-sub");
    gtk_label_set_xalign(GTK_LABEL(c_s), 0.0f);
    gtk_box_append(GTK_BOX(ctrl_col), c_t);
    gtk_box_append(GTK_BOX(ctrl_col), c_s);

    st->clip_state_badge = gtk_label_new("同步中");
    gtk_widget_add_css_class(st->clip_state_badge, "status-pill");
    gtk_widget_add_css_class(st->clip_state_badge, "pill-online");
    gtk_widget_set_valign(st->clip_state_badge, GTK_ALIGN_CENTER);

    st->clip_switch = gtk_switch_new();
    gtk_switch_set_active(GTK_SWITCH(st->clip_switch), TRUE);
    gtk_widget_set_valign(st->clip_switch, GTK_ALIGN_CENTER);
    gtk_widget_set_halign(st->clip_switch, GTK_ALIGN_END);
    gtk_widget_set_cursor_from_name(st->clip_switch, "pointer");
    g_signal_connect(st->clip_switch, "state-set", G_CALLBACK(on_sync_switch_state_set), st);

    gtk_box_append(GTK_BOX(ctrl_card), ctrl_col);
    gtk_box_append(GTK_BOX(ctrl_card), st->clip_state_badge);
    gtk_box_append(GTK_BOX(ctrl_card), st->clip_switch);
    gtk_box_append(GTK_BOX(page), ctrl_card);

    GtkWidget *sum_card = gtk_box_new(GTK_ORIENTATION_VERTICAL, 10);
    gtk_widget_add_css_class(sum_card, "glass-card");
    gtk_widget_set_vexpand(sum_card, FALSE);
    gtk_widget_set_valign(sum_card, GTK_ALIGN_START);
    GtkWidget *sum_t = gtk_label_new("最近一次同步记录");
    gtk_widget_add_css_class(sum_t, "section-title");
    gtk_label_set_xalign(GTK_LABEL(sum_t), 0.0f);
    st->clip_summary_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
    GtkWidget *none_lbl = gtk_label_new("暂无同步记录。在任意已配对设备上复制文本或图片即可自动同步。");
    gtk_widget_add_css_class(none_lbl, "hero-sub");
    gtk_label_set_xalign(GTK_LABEL(none_lbl), 0.0f);
    gtk_box_append(GTK_BOX(st->clip_summary_box), none_lbl);
    gtk_box_append(GTK_BOX(sum_card), sum_t);
    gtk_box_append(GTK_BOX(sum_card), st->clip_summary_box);
    gtk_box_append(GTK_BOX(page), sum_card);
    return page;
}

static GtkWidget *build_transfers_page(WsUiState *st) {
    GtkWidget *page = gtk_box_new(GTK_ORIENTATION_VERTICAL, 16);

    GtkWidget *hdr = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
    gtk_widget_set_vexpand(hdr, FALSE);
    GtkWidget *t = gtk_label_new("文件传输");
    gtk_widget_add_css_class(t, "hero-title");
    gtk_label_set_xalign(GTK_LABEL(t), 0.0f);
    GtkWidget *s = gtk_label_new("直连 mTLS + BLAKE3 校验流式传输，支持多文件并发发送与接收");
    gtk_widget_add_css_class(s, "hero-sub");
    gtk_label_set_xalign(GTK_LABEL(s), 0.0f);
    gtk_box_append(GTK_BOX(hdr), t);
    gtk_box_append(GTK_BOX(hdr), s);
    gtk_box_append(GTK_BOX(page), hdr);

    GtkWidget *bar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 12);
    gtk_widget_add_css_class(bar, "glass-card");
    gtk_widget_set_vexpand(bar, FALSE);
    gtk_widget_set_valign(bar, GTK_ALIGN_START);

    GtkWidget *dir_col = gtk_box_new(GTK_ORIENTATION_VERTICAL, 3);
    gtk_widget_set_hexpand(dir_col, TRUE);
    gtk_widget_set_vexpand(dir_col, FALSE);
    GtkWidget *dl = gtk_label_new("文件接收目录");
    gtk_widget_add_css_class(dl, "section-title");
    gtk_label_set_xalign(GTK_LABEL(dl), 0.0f);
    st->receive_dir_label = gtk_label_new("~/Downloads/寰宇工作台");
    gtk_widget_add_css_class(st->receive_dir_label, "mono-meta");
    gtk_label_set_xalign(GTK_LABEL(st->receive_dir_label), 0.0f);
    gtk_label_set_ellipsize(GTK_LABEL(st->receive_dir_label), PANGO_ELLIPSIZE_MIDDLE);
    gtk_box_append(GTK_BOX(dir_col), dl);
    gtk_box_append(GTK_BOX(dir_col), st->receive_dir_label);

    st->quick_send_bar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
    gtk_widget_set_valign(st->quick_send_bar, GTK_ALIGN_CENTER);

    GtkWidget *open_btn = gtk_button_new();
    gtk_button_set_has_frame(GTK_BUTTON(open_btn), FALSE);
    gtk_widget_add_css_class(open_btn, "glass-btn");
    gtk_widget_set_valign(open_btn, GTK_ALIGN_CENTER);
    gtk_widget_set_cursor_from_name(open_btn, "pointer");
    GtkWidget *ob_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
    gtk_box_append(GTK_BOX(ob_row), make_icon(ICON_FOLDER, 14));
    gtk_box_append(GTK_BOX(ob_row), gtk_label_new("打开目录"));
    gtk_button_set_child(GTK_BUTTON(open_btn), ob_row);
    g_signal_connect(open_btn, "clicked", G_CALLBACK(on_open_downloads_clicked), st);

    gtk_box_append(GTK_BOX(bar), dir_col);
    gtk_box_append(GTK_BOX(bar), st->quick_send_bar);
    gtk_box_append(GTK_BOX(bar), open_btn);
    gtk_box_append(GTK_BOX(page), bar);

    GtkWidget *scroll = gtk_scrolled_window_new();
    gtk_scrolled_window_set_policy(GTK_SCROLLED_WINDOW(scroll), GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
    gtk_widget_set_vexpand(scroll, TRUE);

    GtkWidget *wrap = gtk_box_new(GTK_ORIENTATION_VERTICAL, 12);
    gtk_widget_set_valign(wrap, GTK_ALIGN_START);
    st->transfers_list_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 10);
    st->transfers_empty_card = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
    gtk_widget_add_css_class(st->transfers_empty_card, "glass-card");
    GtkWidget *et = gtk_label_new("暂无文件传输记录");
    gtk_widget_add_css_class(et, "section-title");
    gtk_label_set_xalign(GTK_LABEL(et), 0.0f);
    GtkWidget *es = gtk_label_new("可在上方快捷发送或文件管理器右键菜单发送，手机分享的文件也会显示在这里");
    gtk_widget_add_css_class(es, "hero-sub");
    gtk_label_set_xalign(GTK_LABEL(es), 0.0f);
    gtk_box_append(GTK_BOX(st->transfers_empty_card), et);
    gtk_box_append(GTK_BOX(st->transfers_empty_card), es);

    gtk_box_append(GTK_BOX(wrap), st->transfers_list_box);
    gtk_box_append(GTK_BOX(wrap), st->transfers_empty_card);
    gtk_scrolled_window_set_child(GTK_SCROLLED_WINDOW(scroll), wrap);
    gtk_box_append(GTK_BOX(page), scroll);
    return page;
}

static GtkWidget *build_modal_overlay(WsUiState *st) {
    st->modal_scrim_box = gtk_overlay_new();
    gtk_widget_set_visible(st->modal_scrim_box, FALSE);

    st->modal_scrim_backdrop = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    gtk_widget_add_css_class(st->modal_scrim_backdrop, "modal-scrim");
    gtk_widget_set_hexpand(st->modal_scrim_backdrop, TRUE);
    gtk_widget_set_vexpand(st->modal_scrim_backdrop, TRUE);
    GtkGesture *scrim_click = gtk_gesture_click_new();
    g_signal_connect(scrim_click, "pressed", G_CALLBACK(on_scrim_pressed), st);
    gtk_widget_add_controller(st->modal_scrim_backdrop, GTK_EVENT_CONTROLLER(scrim_click));
    gtk_overlay_set_child(GTK_OVERLAY(st->modal_scrim_box), st->modal_scrim_backdrop);

    st->modal_card = gtk_box_new(GTK_ORIENTATION_VERTICAL, 16);
    gtk_widget_add_css_class(st->modal_card, "modal-card");
    gtk_widget_set_size_request(st->modal_card, 520, -1);
    gtk_widget_set_hexpand(st->modal_card, FALSE);
    gtk_widget_set_vexpand(st->modal_card, FALSE);
    gtk_widget_set_halign(st->modal_card, GTK_ALIGN_CENTER);
    gtk_widget_set_valign(st->modal_card, GTK_ALIGN_CENTER);

    /* Modal header matching Desktop - 3: Title left, circular white X right */
    GtkWidget *m_hdr = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 12);
    gtk_widget_set_vexpand(m_hdr, FALSE);
    st->modal_title_label = gtk_label_new("设置");
    gtk_widget_add_css_class(st->modal_title_label, "modal-title");
    gtk_label_set_xalign(GTK_LABEL(st->modal_title_label), 0.0f);
    gtk_widget_set_hexpand(st->modal_title_label, TRUE);

    GtkWidget *close_btn = gtk_button_new();
    gtk_button_set_has_frame(GTK_BUTTON(close_btn), FALSE);
    gtk_widget_add_css_class(close_btn, "modal-close-btn");
    gtk_widget_set_cursor_from_name(close_btn, "pointer");
    gtk_button_set_child(GTK_BUTTON(close_btn), make_icon(ICON_CLOSE, 15));
    g_signal_connect(close_btn, "clicked", G_CALLBACK(on_close_modal_clicked), st);

    gtk_box_append(GTK_BOX(m_hdr), st->modal_title_label);
    gtk_box_append(GTK_BOX(m_hdr), close_btn);
    gtk_box_append(GTK_BOX(st->modal_card), m_hdr);

    GtkWidget *m_scroll = gtk_scrolled_window_new();
    gtk_scrolled_window_set_policy(GTK_SCROLLED_WINDOW(m_scroll), GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
    gtk_scrolled_window_set_min_content_height(GTK_SCROLLED_WINDOW(m_scroll), 340);
    gtk_scrolled_window_set_max_content_height(GTK_SCROLLED_WINDOW(m_scroll), 460);

    st->modal_body_stack = gtk_stack_new();
    gtk_stack_set_transition_type(GTK_STACK(st->modal_body_stack), GTK_STACK_TRANSITION_TYPE_CROSSFADE);
    gtk_stack_set_transition_duration(GTK_STACK(st->modal_body_stack), 180);

    /* --- Settings Modal Body --- */
    GtkWidget *set_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 12);

    GtkWidget *user_row = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
    gtk_widget_add_css_class(user_row, "setting-row");
    GtkWidget *user_title = gtk_label_new("用户名");
    gtk_widget_add_css_class(user_title, "section-title");
    gtk_label_set_xalign(GTK_LABEL(user_title), 0.0f);
    GtkWidget *user_hint = gtk_label_new("首页问候语显示，留空保存恢复系统用户名");
    gtk_widget_add_css_class(user_hint, "hero-sub");
    gtk_label_set_xalign(GTK_LABEL(user_hint), 0.0f);
    GtkWidget *user_edit_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
    st->user_name_entry = gtk_entry_new();
    gtk_widget_add_css_class(st->user_name_entry, "glass-entry");
    gtk_widget_set_hexpand(st->user_name_entry, TRUE);
    gtk_entry_set_max_length(GTK_ENTRY(st->user_name_entry), 64);
    gtk_entry_set_placeholder_text(GTK_ENTRY(st->user_name_entry), "系统默认用户名");
    g_signal_connect(st->user_name_entry, "activate", G_CALLBACK(on_user_name_entry_activate), st);
    GtkWidget *user_save_btn = gtk_button_new_with_label("保存");
    gtk_button_set_has_frame(GTK_BUTTON(user_save_btn), FALSE);
    gtk_widget_add_css_class(user_save_btn, "glass-btn-primary");
    gtk_widget_set_cursor_from_name(user_save_btn, "pointer");
    g_signal_connect(user_save_btn, "clicked", G_CALLBACK(on_save_user_name_clicked), st);
    gtk_box_append(GTK_BOX(user_edit_row), st->user_name_entry);
    gtk_box_append(GTK_BOX(user_edit_row), user_save_btn);
    gtk_box_append(GTK_BOX(user_row), user_title);
    gtk_box_append(GTK_BOX(user_row), user_hint);
    gtk_box_append(GTK_BOX(user_row), user_edit_row);
    gtk_box_append(GTK_BOX(set_box), user_row);

    GtkWidget *dev_row = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
    gtk_widget_add_css_class(dev_row, "setting-row");
    GtkWidget *dev_title = gtk_label_new("本机名称");
    gtk_widget_add_css_class(dev_title, "section-title");
    gtk_label_set_xalign(GTK_LABEL(dev_title), 0.0f);
    GtkWidget *dev_hint = gtk_label_new("在对端手机中显示的电脑名称，留空保存恢复主机名");
    gtk_widget_add_css_class(dev_hint, "hero-sub");
    gtk_label_set_xalign(GTK_LABEL(dev_hint), 0.0f);
    GtkWidget *dev_edit_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
    st->device_name_entry = gtk_entry_new();
    gtk_widget_add_css_class(st->device_name_entry, "glass-entry");
    gtk_widget_set_hexpand(st->device_name_entry, TRUE);
    gtk_entry_set_max_length(GTK_ENTRY(st->device_name_entry), 64);
    gtk_entry_set_placeholder_text(GTK_ENTRY(st->device_name_entry), "默认主机名");
    g_signal_connect(st->device_name_entry, "activate", G_CALLBACK(on_device_name_entry_activate), st);
    GtkWidget *dev_save_btn = gtk_button_new_with_label("保存");
    gtk_button_set_has_frame(GTK_BUTTON(dev_save_btn), FALSE);
    gtk_widget_add_css_class(dev_save_btn, "glass-btn-primary");
    gtk_widget_set_cursor_from_name(dev_save_btn, "pointer");
    g_signal_connect(dev_save_btn, "clicked", G_CALLBACK(on_save_device_name_clicked), st);
    gtk_box_append(GTK_BOX(dev_edit_row), st->device_name_entry);
    gtk_box_append(GTK_BOX(dev_edit_row), dev_save_btn);
    gtk_box_append(GTK_BOX(dev_row), dev_title);
    gtk_box_append(GTK_BOX(dev_row), dev_hint);
    gtk_box_append(GTK_BOX(dev_row), dev_edit_row);
    gtk_box_append(GTK_BOX(set_box), dev_row);

    GtkWidget *theme_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
    gtk_widget_add_css_class(theme_row, "setting-row");
    GtkWidget *theme_lbl = gtk_label_new("玻璃主题外观");
    gtk_widget_add_css_class(theme_lbl, "section-title");
    gtk_widget_set_hexpand(theme_lbl, TRUE);
    gtk_label_set_xalign(GTK_LABEL(theme_lbl), 0.0f);

    GtkWidget *seg = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
    gtk_widget_add_css_class(seg, "seg-wrap");
    const char *t_names[3] = {"浅色", "深色", "跟随系统"};
    const char *t_keys[3] = {"light", "dark", "system"};
    for (int i = 0; i < 3; ++i) {
        st->theme_btns[i] = gtk_button_new_with_label(t_names[i]);
        gtk_button_set_has_frame(GTK_BUTTON(st->theme_btns[i]), FALSE);
        gtk_widget_add_css_class(st->theme_btns[i], "seg-btn");
        gtk_widget_set_cursor_from_name(st->theme_btns[i], "pointer");
        g_signal_connect_data(st->theme_btns[i], "clicked", G_CALLBACK(on_theme_btn_clicked),
                              id_action_new(st, t_keys[i]), id_action_free, 0);
        gtk_box_append(GTK_BOX(seg), st->theme_btns[i]);
    }
    gtk_box_append(GTK_BOX(theme_row), theme_lbl);
    gtk_box_append(GTK_BOX(theme_row), seg);
    gtk_box_append(GTK_BOX(set_box), theme_row);

    GtkWidget *clip_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
    gtk_widget_add_css_class(clip_row, "setting-row");
    GtkWidget *clip_lbl = gtk_label_new("启用跨端剪贴板实时同步");
    gtk_widget_add_css_class(clip_lbl, "section-title");
    gtk_widget_set_hexpand(clip_lbl, TRUE);
    gtk_label_set_xalign(GTK_LABEL(clip_lbl), 0.0f);
    st->settings_clip_switch = gtk_switch_new();
    gtk_switch_set_active(GTK_SWITCH(st->settings_clip_switch), TRUE);
    gtk_widget_set_valign(st->settings_clip_switch, GTK_ALIGN_CENTER);
    gtk_widget_set_cursor_from_name(st->settings_clip_switch, "pointer");
    g_signal_connect(st->settings_clip_switch, "state-set", G_CALLBACK(on_sync_switch_state_set), st);
    gtk_box_append(GTK_BOX(clip_row), clip_lbl);
    gtk_box_append(GTK_BOX(clip_row), st->settings_clip_switch);
    gtk_box_append(GTK_BOX(set_box), clip_row);

    GtkWidget *dir_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
    gtk_widget_add_css_class(dir_row, "setting-row");
    GtkWidget *dir_col = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
    gtk_widget_set_hexpand(dir_col, TRUE);
    GtkWidget *dl = gtk_label_new("文件接收目录");
    gtk_widget_add_css_class(dl, "section-title");
    gtk_label_set_xalign(GTK_LABEL(dl), 0.0f);
    st->settings_dir_label = gtk_label_new("~/Downloads/寰宇工作台");
    gtk_widget_add_css_class(st->settings_dir_label, "mono-meta");
    gtk_label_set_xalign(GTK_LABEL(st->settings_dir_label), 0.0f);
    gtk_label_set_ellipsize(GTK_LABEL(st->settings_dir_label), PANGO_ELLIPSIZE_MIDDLE);
    gtk_box_append(GTK_BOX(dir_col), dl);
    gtk_box_append(GTK_BOX(dir_col), st->settings_dir_label);
    GtkWidget *s_open_btn = gtk_button_new_with_label("打开目录");
    gtk_button_set_has_frame(GTK_BUTTON(s_open_btn), FALSE);
    gtk_widget_add_css_class(s_open_btn, "glass-btn");
    gtk_widget_set_valign(s_open_btn, GTK_ALIGN_CENTER);
    gtk_widget_set_cursor_from_name(s_open_btn, "pointer");
    g_signal_connect(s_open_btn, "clicked", G_CALLBACK(on_open_downloads_clicked), st);
    gtk_box_append(GTK_BOX(dir_row), dir_col);
    gtk_box_append(GTK_BOX(dir_row), s_open_btn);
    gtk_box_append(GTK_BOX(set_box), dir_row);

    GtkWidget *pm_lbl = gtk_label_new("已配对设备管理");
    gtk_widget_add_css_class(pm_lbl, "section-title");
    gtk_label_set_xalign(GTK_LABEL(pm_lbl), 0.0f);
    gtk_widget_set_margin_top(pm_lbl, 4);
    gtk_box_append(GTK_BOX(set_box), pm_lbl);

    st->settings_peers_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
    st->settings_peers_empty = gtk_label_new("暂无已配对设备");
    gtk_widget_add_css_class(st->settings_peers_empty, "hero-sub");
    gtk_label_set_xalign(GTK_LABEL(st->settings_peers_empty), 0.0f);
    gtk_box_append(GTK_BOX(set_box), st->settings_peers_box);
    gtk_box_append(GTK_BOX(set_box), st->settings_peers_empty);

    GtkWidget *node_lbl = gtk_label_new("本机节点信息");
    gtk_widget_add_css_class(node_lbl, "section-title");
    gtk_label_set_xalign(GTK_LABEL(node_lbl), 0.0f);
    gtk_widget_set_margin_top(node_lbl, 4);
    gtk_box_append(GTK_BOX(set_box), node_lbl);

    GtkWidget *node_row = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
    gtk_widget_add_css_class(node_row, "setting-row");
    GtkWidget *local_ver_label = gtk_label_new("软件版本: " WS_VERSION_DISPLAY " (" WS_VERSION_NAME ")");
    gtk_widget_add_css_class(local_ver_label, "mono-meta");
    gtk_label_set_xalign(GTK_LABEL(local_ver_label), 0.0f);
    st->local_device_label = gtk_label_new("设备名称: --");
    gtk_widget_add_css_class(st->local_device_label, "mono-meta");
    gtk_label_set_xalign(GTK_LABEL(st->local_device_label), 0.0f);
    st->local_fp_label = gtk_label_new("TLS 指纹: --");
    gtk_widget_add_css_class(st->local_fp_label, "mono-meta");
    gtk_label_set_xalign(GTK_LABEL(st->local_fp_label), 0.0f);
    st->local_port_label = gtk_label_new("监听端口: --");
    gtk_widget_add_css_class(st->local_port_label, "mono-meta");
    gtk_label_set_xalign(GTK_LABEL(st->local_port_label), 0.0f);
    gtk_box_append(GTK_BOX(node_row), local_ver_label);
    gtk_box_append(GTK_BOX(node_row), st->local_device_label);
    gtk_box_append(GTK_BOX(node_row), st->local_fp_label);
    gtk_box_append(GTK_BOX(node_row), st->local_port_label);
    gtk_box_append(GTK_BOX(set_box), node_row);

    GtkWidget *quit_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
    gtk_widget_set_margin_top(quit_row, 8);
    GtkWidget *quit_sp = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
    gtk_widget_set_hexpand(quit_sp, TRUE);
    GtkWidget *quit_btn = gtk_button_new_with_label("完全退出后台服务");
    gtk_button_set_has_frame(GTK_BUTTON(quit_btn), FALSE);
    gtk_widget_add_css_class(quit_btn, "glass-btn-danger");
    gtk_widget_set_cursor_from_name(quit_btn, "pointer");
    g_signal_connect(quit_btn, "clicked", G_CALLBACK(on_quit_clicked), st);
    gtk_box_append(GTK_BOX(quit_row), quit_sp);
    gtk_box_append(GTK_BOX(quit_row), quit_btn);
    gtk_box_append(GTK_BOX(set_box), quit_row);

    gtk_stack_add_named(GTK_STACK(st->modal_body_stack), set_box, "settings");

    /* --- Discover Modal Body --- */
    GtkWidget *disc_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 12);
    GtkWidget *d_hint = gtk_label_new("确保手机端「寰宇工作台」处于同一局域网并已开启局域网发现。");
    gtk_widget_add_css_class(d_hint, "hero-sub");
    gtk_label_set_xalign(GTK_LABEL(d_hint), 0.0f);
    gtk_label_set_wrap(GTK_LABEL(d_hint), TRUE);
    gtk_box_append(GTK_BOX(disc_box), d_hint);

    st->discover_list_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 10);
    st->discover_empty_card = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
    gtk_widget_add_css_class(st->discover_empty_card, "setting-row");
    GtkWidget *de_lbl = gtk_label_new("正在广播局域网发现信标，暂未发现其他在线设备...");
    gtk_widget_add_css_class(de_lbl, "hero-sub");
    gtk_label_set_xalign(GTK_LABEL(de_lbl), 0.0f);
    gtk_box_append(GTK_BOX(st->discover_empty_card), de_lbl);

    gtk_box_append(GTK_BOX(disc_box), st->discover_list_box);
    gtk_box_append(GTK_BOX(disc_box), st->discover_empty_card);
    gtk_stack_add_named(GTK_STACK(st->modal_body_stack), disc_box, "discover");

    gtk_scrolled_window_set_child(GTK_SCROLLED_WINDOW(m_scroll), st->modal_body_stack);
    gtk_box_append(GTK_BOX(st->modal_card), m_scroll);
    gtk_overlay_add_overlay(GTK_OVERLAY(st->modal_scrim_box), st->modal_card);
    return st->modal_scrim_box;
}

/* ---------------- Application Lifecycle ---------------- */

static void on_activate(GtkApplication *app, gpointer data) {
    WsUiState *st = (WsUiState *)data;
    if (st->window) {
        if (!st->start_silent) {
            gtk_window_present(GTK_WINDOW(st->window));
        }
        return;
    }

    st->css_provider = gtk_css_provider_new();
    gtk_css_provider_load_from_string(st->css_provider, GLASS_CSS);
    GdkDisplay *disp = gdk_display_get_default();
    if (disp) {
        gtk_style_context_add_provider_for_display(
            disp, GTK_STYLE_PROVIDER(st->css_provider), GTK_STYLE_PROVIDER_PRIORITY_USER);
    }
    GtkSettings *gs = gtk_settings_get_default();
    if (gs) {
        g_object_set(gs, "gtk-enable-animations", TRUE, NULL);
    }

    st->window = gtk_application_window_new(app);
    gtk_window_set_title(GTK_WINDOW(st->window), "寰宇工作台");
    gtk_window_set_icon_name(GTK_WINDOW(st->window), "me.edfork.workspace");
    gtk_window_set_default_size(GTK_WINDOW(st->window), 1120, 720);
    gtk_widget_set_size_request(st->window, 880, 560);
    gtk_window_set_decorated(GTK_WINDOW(st->window), FALSE);
    gtk_window_set_hide_on_close(GTK_WINDOW(st->window), TRUE);
    gtk_widget_add_css_class(st->window, "glass-win");

    GtkEventController *key_ctrl = gtk_event_controller_key_new();
    g_signal_connect(key_ctrl, "key-pressed", G_CALLBACK(on_window_key_pressed), st);
    gtk_widget_add_controller(st->window, key_ctrl);

    st->root_overlay = gtk_overlay_new();
    gtk_window_set_child(GTK_WINDOW(st->window), st->root_overlay);

    st->stage_overlay = gtk_overlay_new();
    gtk_widget_add_css_class(st->stage_overlay, "glass-shell");
    gtk_widget_set_overflow(st->stage_overlay, GTK_OVERFLOW_HIDDEN);

    st->main_hbox = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
    gtk_widget_set_hexpand(st->main_hbox, TRUE);
    gtk_widget_set_vexpand(st->main_hbox, TRUE);

    GtkWidget *sidebar = build_sidebar(st);
    gtk_box_append(GTK_BOX(st->main_hbox), sidebar);

    GtkWidget *stage_right = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
    gtk_widget_add_css_class(stage_right, "main-stage");
    gtk_widget_set_hexpand(stage_right, TRUE);
    gtk_widget_set_vexpand(stage_right, TRUE);

    /* Subtle top drag strip + hide-to-tray button */
    GtkWidget *top_bar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
    gtk_widget_set_vexpand(top_bar, FALSE);
    GtkWidget *top_drag = gtk_window_handle_new();
    gtk_widget_set_hexpand(top_drag, TRUE);
    GtkWidget *top_sp = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
    gtk_widget_set_size_request(top_sp, -1, 18);
    gtk_window_handle_set_child(GTK_WINDOW_HANDLE(top_drag), top_sp);

    GtkWidget *hide_btn = gtk_button_new();
    gtk_button_set_has_frame(GTK_BUTTON(hide_btn), FALSE);
    gtk_widget_add_css_class(hide_btn, "win-hide-btn");
    gtk_widget_set_tooltip_text(hide_btn, "最小化到系统托盘");
    gtk_widget_set_cursor_from_name(hide_btn, "pointer");
    gtk_button_set_child(GTK_BUTTON(hide_btn), make_icon(ICON_HIDE, 14));
    g_signal_connect(hide_btn, "clicked", G_CALLBACK(on_hide_window_clicked), st);

    gtk_box_append(GTK_BOX(top_bar), top_drag);
    gtk_box_append(GTK_BOX(top_bar), hide_btn);
    gtk_box_append(GTK_BOX(stage_right), top_bar);

    st->content_stack = gtk_stack_new();
    gtk_stack_set_transition_type(GTK_STACK(st->content_stack), GTK_STACK_TRANSITION_TYPE_CROSSFADE);
    gtk_stack_set_transition_duration(GTK_STACK(st->content_stack), 190);
    gtk_widget_set_hexpand(st->content_stack, TRUE);
    gtk_widget_set_vexpand(st->content_stack, TRUE);

    st->pages[0] = build_devices_page(st);
    st->pages[1] = build_clipboard_page(st);
    st->pages[2] = build_transfers_page(st);
    st->pages[3] = build_construction_page("屏幕共享", "SCrcpy 集成",
                                           "无线 ADB 低延迟镜像与跨屏键鼠协同通道接入中");
    st->pages[4] = build_construction_page("OPPO 笔记", "OPPO 笔记同步",
                                           "跨端笔记实时双向同步与富文本解析模块开发中");
    gtk_stack_add_named(GTK_STACK(st->content_stack), st->pages[0], "devices");
    gtk_stack_add_named(GTK_STACK(st->content_stack), st->pages[1], "clipboard");
    gtk_stack_add_named(GTK_STACK(st->content_stack), st->pages[2], "transfers");
    gtk_stack_add_named(GTK_STACK(st->content_stack), st->pages[3], "screen");
    gtk_stack_add_named(GTK_STACK(st->content_stack), st->pages[4], "notes");

    gtk_box_append(GTK_BOX(stage_right), st->content_stack);
    gtk_box_append(GTK_BOX(st->main_hbox), stage_right);

    gtk_overlay_set_child(GTK_OVERLAY(st->stage_overlay), st->main_hbox);

    /* Floating '+' Action Button matching Desktop - 1 */
    st->fab_btn = gtk_button_new();
    gtk_button_set_has_frame(GTK_BUTTON(st->fab_btn), FALSE);
    gtk_widget_add_css_class(st->fab_btn, "fab-btn");
    gtk_widget_set_halign(st->fab_btn, GTK_ALIGN_END);
    gtk_widget_set_valign(st->fab_btn, GTK_ALIGN_END);
    gtk_widget_set_margin_end(st->fab_btn, 32);
    gtk_widget_set_margin_bottom(st->fab_btn, 28);
    gtk_widget_set_tooltip_text(st->fab_btn, "添加同一局域网设备");
    gtk_widget_set_cursor_from_name(st->fab_btn, "pointer");
    gtk_button_set_child(GTK_BUTTON(st->fab_btn), make_icon(ICON_PLUS, 22));
    g_signal_connect(st->fab_btn, "clicked", G_CALLBACK(on_open_discover_clicked), st);
    gtk_overlay_add_overlay(GTK_OVERLAY(st->stage_overlay), st->fab_btn);

    gtk_overlay_set_child(GTK_OVERLAY(st->root_overlay), st->stage_overlay);
    gtk_overlay_add_overlay(GTK_OVERLAY(st->root_overlay), build_modal_overlay(st));

    set_active_page(st, PAGE_DEVICES);
    apply_theme(st);

    if (st->start_silent) {
        gtk_widget_set_visible(st->window, FALSE);
        st->start_silent = 0;
    } else {
        gtk_window_present(GTK_WINDOW(st->window));
    }

    if (st->cb.on_startup_ready) {
        st->cb.on_startup_ready(st->user_data);
    }
}

/* ---------------- Public C ABI for Rust ---------------- */

WsUiState *ws_ui_new(int start_silent, const char *display_user,
                     const unsigned char *logo_png, size_t logo_len,
                     WsUiCallbacks cb, void *user_data) {
    g_setenv("GSK_RENDERER", "gl", FALSE);
    g_set_prgname("me.edfork.workspace");

    WsUiState *st = g_new0(WsUiState, 1);
    st->start_silent = start_silent;
    st->display_user = g_strdup(display_user ? display_user : "User");
    st->theme_mode = THEME_LIGHT;
    st->cb = cb;
    st->user_data = user_data;

    if (logo_png && logo_len > 0) {
        GBytes *bytes = g_bytes_new(logo_png, logo_len);
        GError *err = NULL;
        GdkTexture *tex = gdk_texture_new_from_bytes(bytes, &err);
        if (err) g_error_free(err);
        g_bytes_unref(bytes);
        if (tex) {
            int tw = gdk_texture_get_width(tex);
            int th = gdk_texture_get_height(tex);
            if (tw > 0 && th > 0) {
                cairo_surface_t *surf = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, tw, th);
                int stride = cairo_image_surface_get_stride(surf);
                gdk_texture_download(tex, cairo_image_surface_get_data(surf), (gsize)stride);
                cairo_surface_mark_dirty(surf);
                st->logo_surface = surf;
            }
            g_object_unref(tex);
        }
    }

    st->app = gtk_application_new("me.edfork.workspace", G_APPLICATION_NON_UNIQUE);
    g_signal_connect(st->app, "activate", G_CALLBACK(on_activate), st);
    return st;
}

int ws_ui_run(WsUiState *st) {
    g_application_hold(G_APPLICATION(st->app));
    int rc = g_application_run(G_APPLICATION(st->app), 0, NULL);
    cancel_ui_timers(st);
    if (st->logo_surface) {
        cairo_surface_destroy(st->logo_surface);
        st->logo_surface = NULL;
    }
    g_free(st->display_user);
    g_free(st->current_device_name);
    g_object_unref(st->app);
    g_free(st);
    return rc;
}

void ws_ui_present(WsUiState *st, const char *startup_id) {
    if (!st || !st->window) return;
    int was_hidden = !gtk_widget_get_visible(st->window);
    if (startup_id && startup_id[0]) {
        gtk_window_set_startup_id(GTK_WINDOW(st->window), startup_id);
    }
    refresh_greeting(st);
    gtk_widget_set_visible(st->window, TRUE);
    gtk_window_present(GTK_WINDOW(st->window));
    if (was_hidden && st->stage_overlay) {
        if (st->win_anim_source_id) {
            g_source_remove(st->win_anim_source_id);
            st->win_anim_source_id = 0;
        }
        gtk_widget_remove_css_class(st->stage_overlay, "win-presenting");
        gtk_widget_add_css_class(st->stage_overlay, "win-presenting");
        st->win_anim_source_id = g_timeout_add(220, win_present_finish_cb, st);
    }
}

void ws_ui_quit(WsUiState *st) {
    if (!st || st->quit_requested) return;
    st->quit_requested = 1;
    cancel_ui_timers(st);
    if (st->window) {
        gtk_window_destroy(GTK_WINDOW(st->window));
        st->window = NULL;
    }
    g_application_release(G_APPLICATION(st->app));
    g_application_quit(G_APPLICATION(st->app));
}

void ws_ui_set_header(WsUiState *st, const char *user_name, const char *device_name,
                      const char *fingerprint, int tls_port, const char *receive_dir,
                      int sync_paused, int online_count) {
    if (!st || !st->window) return;
    if (user_name && user_name[0]) {
        g_free(st->display_user);
        st->display_user = g_strdup(user_name);
    }
    if (device_name) {
        g_free(st->current_device_name);
        st->current_device_name = g_strdup(device_name);
    }
    refresh_greeting(st);

    if (st->user_name_entry && !entry_has_active_focus(st, st->user_name_entry)) {
        gtk_editable_set_text(GTK_EDITABLE(st->user_name_entry), st->display_user ? st->display_user : "");
    }
    if (st->device_name_entry && !entry_has_active_focus(st, st->device_name_entry)) {
        gtk_editable_set_text(GTK_EDITABLE(st->device_name_entry), st->current_device_name ? st->current_device_name : "");
    }

    char badge_txt[96];
    if (online_count > 0) {
        snprintf(badge_txt, sizeof(badge_txt), "%d 台已连接 · TLS :%d", online_count, tls_port);
    } else {
        snprintf(badge_txt, sizeof(badge_txt), "等待设备 · TLS :%d", tls_port);
    }
    gtk_label_set_text(GTK_LABEL(st->lan_status_badge), badge_txt);

    st->sync_paused = sync_paused;
    st->updating_sync_switch = 1;
    gtk_switch_set_active(GTK_SWITCH(st->clip_switch), !sync_paused);
    gtk_switch_set_active(GTK_SWITCH(st->settings_clip_switch), !sync_paused);
    st->updating_sync_switch = 0;

    gtk_widget_remove_css_class(st->clip_state_badge, "pill-online");
    gtk_widget_remove_css_class(st->clip_state_badge, "pill-amber");
    if (sync_paused) {
        gtk_label_set_text(GTK_LABEL(st->clip_state_badge), "已暂停");
        gtk_widget_add_css_class(st->clip_state_badge, "pill-amber");
    } else {
        gtk_label_set_text(GTK_LABEL(st->clip_state_badge), "同步中");
        gtk_widget_add_css_class(st->clip_state_badge, "pill-online");
    }

    gtk_label_set_text(GTK_LABEL(st->receive_dir_label), receive_dir ? receive_dir : "");
    gtk_label_set_text(GTK_LABEL(st->settings_dir_label), receive_dir ? receive_dir : "");

    char buf[256];
    snprintf(buf, sizeof(buf), "设备名称: %s", device_name ? device_name : "--");
    gtk_label_set_text(GTK_LABEL(st->local_device_label), buf);
    snprintf(buf, sizeof(buf), "TLS 指纹: %s", fingerprint ? fingerprint : "--");
    gtk_label_set_text(GTK_LABEL(st->local_fp_label), buf);
    snprintf(buf, sizeof(buf), "监听端口: %d", tls_port);
    gtk_label_set_text(GTK_LABEL(st->local_port_label), buf);
}

void ws_ui_clear_sas(WsUiState *st) {
    if (!st || !st->sas_container) return;
    clear_container(st->sas_container);
    if (st->sas_revealer) {
        gtk_revealer_set_reveal_child(GTK_REVEALER(st->sas_revealer), FALSE);
    }
}

void ws_ui_add_sas(WsUiState *st, const char *session_id, const char *peer_name,
                   const char *fingerprint, const char *sas_code,
                   int is_initiator, int approved_local) {
    if (!st || !st->sas_container) return;
    if (st->sas_revealer) {
        gtk_revealer_set_reveal_child(GTK_REVEALER(st->sas_revealer), TRUE);
    }

    GtkWidget *card = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 16);
    gtk_widget_add_css_class(card, "sas-card");
    gtk_widget_add_css_class(card, "card-enter");

    GtkWidget *left = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
    gtk_widget_set_hexpand(left, TRUE);
    char title[256];
    snprintf(title, sizeof(title), "%s：%s",
             is_initiator ? "待手机确认配对" : "收到配对请求",
             peer_name ? peer_name : "");
    GtkWidget *t_lbl = gtk_label_new(title);
    gtk_widget_add_css_class(t_lbl, "card-title");
    gtk_label_set_xalign(GTK_LABEL(t_lbl), 0.0f);

    char sub[256];
    snprintf(sub, sizeof(sub), "请核对两端 6 位数字验证码一致 · 指纹 %s", fingerprint ? fingerprint : "");
    GtkWidget *s_lbl = gtk_label_new(sub);
    gtk_widget_add_css_class(s_lbl, "mono-meta");
    gtk_label_set_xalign(GTK_LABEL(s_lbl), 0.0f);
    gtk_box_append(GTK_BOX(left), t_lbl);
    gtk_box_append(GTK_BOX(left), s_lbl);

    GtkWidget *code_lbl = gtk_label_new(sas_code ? sas_code : "");
    gtk_widget_add_css_class(code_lbl, "sas-code");
    gtk_widget_set_valign(code_lbl, GTK_ALIGN_CENTER);

    GtkWidget *acts = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
    gtk_widget_set_valign(acts, GTK_ALIGN_CENTER);
    if (!approved_local) {
        GtkWidget *ok_btn = gtk_button_new_with_label("确认配对");
        gtk_button_set_has_frame(GTK_BUTTON(ok_btn), FALSE);
        gtk_widget_add_css_class(ok_btn, "glass-btn-primary");
        gtk_widget_set_cursor_from_name(ok_btn, "pointer");
        g_signal_connect_data(ok_btn, "clicked", G_CALLBACK(on_pair_approve_clicked),
                              id_action_new(st, session_id), id_action_free, 0);
        gtk_box_append(GTK_BOX(acts), ok_btn);
    } else {
        GtkWidget *wait_lbl = gtk_label_new("等待手机确认");
        gtk_widget_add_css_class(wait_lbl, "status-pill");
        gtk_widget_add_css_class(wait_lbl, "pill-amber");
        gtk_box_append(GTK_BOX(acts), wait_lbl);
    }

    GtkWidget *rej_btn = gtk_button_new_with_label("拒绝");
    gtk_button_set_has_frame(GTK_BUTTON(rej_btn), FALSE);
    gtk_widget_add_css_class(rej_btn, "glass-btn-danger");
    gtk_widget_set_cursor_from_name(rej_btn, "pointer");
    g_signal_connect_data(rej_btn, "clicked", G_CALLBACK(on_pair_reject_clicked),
                          id_action_new(st, session_id), id_action_free, 0);
    gtk_box_append(GTK_BOX(acts), rej_btn);

    gtk_box_append(GTK_BOX(card), left);
    gtk_box_append(GTK_BOX(card), code_lbl);
    gtk_box_append(GTK_BOX(card), acts);
    gtk_box_append(GTK_BOX(st->sas_container), card);
}

void ws_ui_clear_paired(WsUiState *st) {
    if (!st) return;
    clear_container(st->paired_flow);
    clear_container(st->settings_peers_box);
    clear_container(st->quick_send_bar);
    memset(st->paired_hero_labels, 0, sizeof(st->paired_hero_labels));
    memset(st->paired_cap_labels, 0, sizeof(st->paired_cap_labels));
    st->paired_count = 0;
    st->online_peer_count = 0;
}

void ws_ui_add_paired(WsUiState *st, const char *device_id, const char *device_name,
                      int connected, const char *ip, int tls_port, int64_t rtt_ms,
                      const char *fingerprint) {
    if (!st || !st->paired_flow) return;
    int idx = st->paired_count;
    st->paired_count++;
    if (connected) st->online_peer_count++;

    /* --- 1. Prototype Device Card (Desktop - 1, strictly 360x196) --- */
    GtkWidget *card = gtk_box_new(GTK_ORIENTATION_VERTICAL, 10);
    gtk_widget_add_css_class(card, "glass-card");
    gtk_widget_add_css_class(card, "device-card");
    gtk_widget_add_css_class(card, "card-enter");
    gtk_widget_set_size_request(card, 360, 196);
    gtk_widget_set_hexpand(card, FALSE);
    gtk_widget_set_vexpand(card, FALSE);

    GtkWidget *top = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
    GtkWidget *name_lbl = gtk_label_new(device_name ? device_name : "");
    gtk_widget_add_css_class(name_lbl, "card-title");
    gtk_label_set_xalign(GTK_LABEL(name_lbl), 0.0f);
    gtk_label_set_ellipsize(GTK_LABEL(name_lbl), PANGO_ELLIPSIZE_END);
    gtk_widget_set_hexpand(name_lbl, TRUE);

    GtkWidget *send_btn = gtk_button_new();
    gtk_button_set_has_frame(GTK_BUTTON(send_btn), FALSE);
    gtk_widget_add_css_class(send_btn, "glass-btn");
    gtk_widget_set_cursor_from_name(send_btn, "pointer");
    GtkWidget *sb_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 5);
    gtk_box_append(GTK_BOX(sb_row), make_icon(ICON_SEND, 13));
    gtk_box_append(GTK_BOX(sb_row), gtk_label_new("发送文件"));
    gtk_button_set_child(GTK_BUTTON(send_btn), sb_row);
    gtk_widget_set_sensitive(send_btn, connected != 0);
    g_signal_connect_data(send_btn, "clicked", G_CALLBACK(on_pick_files_clicked),
                          id_action_new(st, device_id), id_action_free, 0);

    gtk_box_append(GTK_BOX(top), name_lbl);
    gtk_box_append(GTK_BOX(top), send_btn);
    gtk_box_append(GTK_BOX(card), top);

    GtkWidget *badge_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
    GtkWidget *status_pill = gtk_label_new(connected ? "已连接" : "离线");
    gtk_widget_add_css_class(status_pill, "status-pill");
    gtk_widget_add_css_class(status_pill, connected ? "pill-online" : "pill-offline");
    gtk_box_append(GTK_BOX(badge_row), status_pill);

    char meta[192];
    if (connected && ip && ip[0]) {
        snprintf(meta, sizeof(meta), "%s:%d · %.8s", ip, tls_port, fingerprint ? fingerprint : "");
    } else {
        snprintf(meta, sizeof(meta), "指纹 %.12s", fingerprint ? fingerprint : "");
    }
    GtkWidget *meta_lbl = gtk_label_new(meta);
    gtk_widget_add_css_class(meta_lbl, "mono-meta");
    gtk_label_set_ellipsize(GTK_LABEL(meta_lbl), PANGO_ELLIPSIZE_END);
    gtk_box_append(GTK_BOX(badge_row), meta_lbl);
    gtk_box_append(GTK_BOX(card), badge_row);

    /* Signature giant bottom-right numeric telemetry from Desktop - 1 */
    GtkWidget *sp = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    gtk_widget_set_vexpand(sp, TRUE);
    gtk_box_append(GTK_BOX(card), sp);

    GtkWidget *metric_col = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    gtk_widget_set_halign(metric_col, GTK_ALIGN_END);

    GtkWidget *hero_lbl = gtk_label_new("");
    gtk_widget_add_css_class(hero_lbl, "metric-hero");
    gtk_widget_set_halign(hero_lbl, GTK_ALIGN_END);

    GtkWidget *cap_lbl = gtk_label_new("");
    gtk_widget_add_css_class(cap_lbl, "metric-caption");
    gtk_widget_set_halign(cap_lbl, GTK_ALIGN_END);

    apply_rtt_to_labels(hero_lbl, cap_lbl, connected, rtt_ms);
    if (idx >= 0 && idx < MAX_PAIRED_CARDS) {
        st->paired_hero_labels[idx] = hero_lbl;
        st->paired_cap_labels[idx] = cap_lbl;
    }

    gtk_box_append(GTK_BOX(metric_col), hero_lbl);
    gtk_box_append(GTK_BOX(metric_col), cap_lbl);
    gtk_box_append(GTK_BOX(card), metric_col);

    gtk_flow_box_append(GTK_FLOW_BOX(st->paired_flow), card);

    /* --- 2. Quick-send pill on Transfers page if online --- */
    if (connected) {
        char qlabel[160];
        snprintf(qlabel, sizeof(qlabel), "向 %s 发送", device_name ? device_name : "");
        GtkWidget *qbtn = gtk_button_new_with_label(qlabel);
        gtk_button_set_has_frame(GTK_BUTTON(qbtn), FALSE);
        gtk_widget_add_css_class(qbtn, "glass-btn-primary");
        gtk_widget_set_cursor_from_name(qbtn, "pointer");
        g_signal_connect_data(qbtn, "clicked", G_CALLBACK(on_pick_files_clicked),
                              id_action_new(st, device_id), id_action_free, 0);
        gtk_box_append(GTK_BOX(st->quick_send_bar), qbtn);
    }

    /* --- 3. Settings Modal paired peer management row --- */
    GtkWidget *s_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
    gtk_widget_add_css_class(s_row, "setting-row");
    GtkWidget *s_col = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
    gtk_widget_set_hexpand(s_col, TRUE);
    GtkWidget *s_name = gtk_label_new(device_name ? device_name : "");
    gtk_widget_add_css_class(s_name, "section-title");
    gtk_label_set_xalign(GTK_LABEL(s_name), 0.0f);
    GtkWidget *s_fp = gtk_label_new(meta);
    gtk_widget_add_css_class(s_fp, "mono-meta");
    gtk_label_set_xalign(GTK_LABEL(s_fp), 0.0f);
    gtk_box_append(GTK_BOX(s_col), s_name);
    gtk_box_append(GTK_BOX(s_col), s_fp);

    GtkWidget *unpair_btn = gtk_button_new_with_label("取消配对");
    gtk_button_set_has_frame(GTK_BUTTON(unpair_btn), FALSE);
    gtk_widget_add_css_class(unpair_btn, "glass-btn-danger");
    gtk_widget_set_valign(unpair_btn, GTK_ALIGN_CENTER);
    gtk_widget_set_cursor_from_name(unpair_btn, "pointer");
    g_signal_connect_data(unpair_btn, "clicked", G_CALLBACK(on_unpair_clicked),
                          id_action_new(st, device_id), id_action_free, 0);

    gtk_box_append(GTK_BOX(s_row), s_col);
    gtk_box_append(GTK_BOX(s_row), unpair_btn);
    gtk_box_append(GTK_BOX(st->settings_peers_box), s_row);
}

void ws_ui_update_paired_rtt(WsUiState *st, int index, int connected, int64_t rtt_ms) {
    if (!st || index < 0 || index >= MAX_PAIRED_CARDS) return;
    apply_rtt_to_labels(st->paired_hero_labels[index], st->paired_cap_labels[index], connected, rtt_ms);
}

void ws_ui_finish_paired(WsUiState *st) {
    if (!st) return;
    int has = st->paired_count > 0;
    gtk_widget_set_visible(st->paired_flow, has);
    gtk_widget_set_visible(st->paired_empty_card, !has);
    gtk_widget_set_visible(st->settings_peers_box, has);
    gtk_widget_set_visible(st->settings_peers_empty, !has);
    gtk_widget_set_visible(st->quick_send_bar, st->online_peer_count > 0);
}

void ws_ui_clear_discovered(WsUiState *st) {
    if (!st) return;
    clear_container(st->discover_list_box);
    st->discovered_count = 0;
}

void ws_ui_add_discovered(WsUiState *st, const char *device_id, const char *device_name,
                          const char *ip, int tls_port, const char *fingerprint, int is_trusted) {
    if (!st || !st->discover_list_box) return;
    st->discovered_count++;

    GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 12);
    gtk_widget_add_css_class(row, "setting-row");
    gtk_widget_add_css_class(row, "card-enter");

    GtkWidget *col = gtk_box_new(GTK_ORIENTATION_VERTICAL, 3);
    gtk_widget_set_hexpand(col, TRUE);
    GtkWidget *n = gtk_label_new(device_name ? device_name : "");
    gtk_widget_add_css_class(n, "section-title");
    gtk_label_set_xalign(GTK_LABEL(n), 0.0f);

    char sub[192];
    snprintf(sub, sizeof(sub), "%s:%d · %.10s", ip ? ip : "", tls_port, fingerprint ? fingerprint : "");
    GtkWidget *m = gtk_label_new(sub);
    gtk_widget_add_css_class(m, "mono-meta");
    gtk_label_set_xalign(GTK_LABEL(m), 0.0f);
    gtk_box_append(GTK_BOX(col), n);
    gtk_box_append(GTK_BOX(col), m);

    GtkWidget *btn = gtk_button_new_with_label(is_trusted ? "已配对" : "发起 6 位码配对");
    gtk_button_set_has_frame(GTK_BUTTON(btn), FALSE);
    gtk_widget_add_css_class(btn, is_trusted ? "glass-btn" : "glass-btn-primary");
    gtk_widget_set_valign(btn, GTK_ALIGN_CENTER);
    gtk_widget_set_sensitive(btn, !is_trusted);
    if (!is_trusted) {
        gtk_widget_set_cursor_from_name(btn, "pointer");
        g_signal_connect_data(btn, "clicked", G_CALLBACK(on_pair_start_clicked),
                              id_action_new(st, device_id), id_action_free, 0);
    }

    gtk_box_append(GTK_BOX(row), col);
    gtk_box_append(GTK_BOX(row), btn);
    gtk_box_append(GTK_BOX(st->discover_list_box), row);
}

void ws_ui_finish_discovered(WsUiState *st) {
    if (!st) return;
    int has = st->discovered_count > 0;
    gtk_widget_set_visible(st->discover_list_box, has);
    gtk_widget_set_visible(st->discover_empty_card, !has);
}

void ws_ui_set_clipboard_summary(WsUiState *st, int has_summary, const char *direction,
                                 const char *peer_name, const char *kind, const char *size_label) {
    if (!st || !st->clip_summary_box) return;
    clear_container(st->clip_summary_box);
    if (!has_summary) {
        GtkWidget *none = gtk_label_new("暂无同步记录。在任意已配对设备上复制文本或图片即可自动同步。");
        gtk_widget_add_css_class(none, "hero-sub");
        gtk_label_set_xalign(GTK_LABEL(none), 0.0f);
        gtk_box_append(GTK_BOX(st->clip_summary_box), none);
        return;
    }

    GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
    GtkWidget *pill = gtk_label_new(direction ? direction : "");
    gtk_widget_add_css_class(pill, "status-pill");
    gtk_widget_add_css_class(pill, "pill-info");

    char info[256];
    snprintf(info, sizeof(info), "对端 %s · 类型 %s · 大小 %s",
             peer_name ? peer_name : "", kind ? kind : "", size_label ? size_label : "");
    GtkWidget *lbl = gtk_label_new(info);
    gtk_widget_add_css_class(lbl, "section-title");
    gtk_label_set_xalign(GTK_LABEL(lbl), 0.0f);

    gtk_box_append(GTK_BOX(row), pill);
    gtk_box_append(GTK_BOX(row), lbl);
    gtk_box_append(GTK_BOX(st->clip_summary_box), row);
}

void ws_ui_clear_transfers(WsUiState *st) {
    if (!st) return;
    clear_container(st->transfers_list_box);
    st->transfer_count = 0;
}

void ws_ui_add_transfer(WsUiState *st, const char *summary, const char *peer_name,
                        const char *direction, const char *bytes_label,
                        const char *state_label, const char *state_kind,
                        double fraction, const char *error_msg) {
    if (!st || !st->transfers_list_box) return;
    st->transfer_count++;

    GtkWidget *card = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
    gtk_widget_add_css_class(card, "glass-card");

    GtkWidget *top = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
    GtkWidget *dir_pill = gtk_label_new(direction ? direction : "");
    gtk_widget_add_css_class(dir_pill, "status-pill");
    gtk_widget_add_css_class(dir_pill, "pill-info");

    GtkWidget *sum_lbl = gtk_label_new(summary ? summary : "");
    gtk_widget_add_css_class(sum_lbl, "section-title");
    gtk_label_set_xalign(GTK_LABEL(sum_lbl), 0.0f);
    gtk_label_set_ellipsize(GTK_LABEL(sum_lbl), PANGO_ELLIPSIZE_END);
    gtk_widget_set_hexpand(sum_lbl, TRUE);

    GtkWidget *st_pill = gtk_label_new(state_label ? state_label : "");
    gtk_widget_add_css_class(st_pill, "status-pill");
    if (state_kind && strcmp(state_kind, "ok") == 0) gtk_widget_add_css_class(st_pill, "pill-online");
    else if (state_kind && strcmp(state_kind, "err") == 0) gtk_widget_add_css_class(st_pill, "pill-danger");
    else if (state_kind && strcmp(state_kind, "run") == 0) gtk_widget_add_css_class(st_pill, "pill-amber");
    else gtk_widget_add_css_class(st_pill, "pill-offline");

    gtk_box_append(GTK_BOX(top), dir_pill);
    gtk_box_append(GTK_BOX(top), sum_lbl);
    gtk_box_append(GTK_BOX(top), st_pill);
    gtk_box_append(GTK_BOX(card), top);

    char sub[256];
    snprintf(sub, sizeof(sub), "%s · %s", peer_name ? peer_name : "", bytes_label ? bytes_label : "");
    GtkWidget *sub_lbl = gtk_label_new(sub);
    gtk_widget_add_css_class(sub_lbl, "mono-meta");
    gtk_label_set_xalign(GTK_LABEL(sub_lbl), 0.0f);
    gtk_box_append(GTK_BOX(card), sub_lbl);

    GtkWidget *pbar = gtk_progress_bar_new();
    if (fraction < 0.0) fraction = 0.0;
    if (fraction > 1.0) fraction = 1.0;
    gtk_progress_bar_set_fraction(GTK_PROGRESS_BAR(pbar), fraction);
    gtk_box_append(GTK_BOX(card), pbar);

    if (error_msg && error_msg[0]) {
        GtkWidget *err_lbl = gtk_label_new(error_msg);
        gtk_widget_add_css_class(err_lbl, "pill-danger");
        gtk_widget_add_css_class(err_lbl, "status-pill");
        gtk_label_set_xalign(GTK_LABEL(err_lbl), 0.0f);
        gtk_box_append(GTK_BOX(card), err_lbl);
    }

    gtk_box_append(GTK_BOX(st->transfers_list_box), card);
}

void ws_ui_finish_transfers(WsUiState *st) {
    if (!st) return;
    int has = st->transfer_count > 0;
    gtk_widget_set_visible(st->transfers_list_box, has);
    gtk_widget_set_visible(st->transfers_empty_card, !has);
}

typedef struct {
    void (*func)(void *);
    void *arg;
} IdleCall;

static gboolean idle_trampoline(gpointer data) {
    IdleCall *ic = (IdleCall *)data;
    ic->func(ic->arg);
    g_free(ic);
    return G_SOURCE_REMOVE;
}

void ws_schedule_idle(void (*func)(void *), void *arg) {
    IdleCall *ic = g_new0(IdleCall, 1);
    ic->func = func;
    ic->arg = arg;
    g_idle_add(idle_trampoline, ic);
}
