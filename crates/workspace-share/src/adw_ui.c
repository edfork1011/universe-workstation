#include <adwaita.h>
#include <gtk/gtk.h>
#include <stdlib.h>
#include <string.h>

typedef void (*ws_share_activate_cb)(void *user_data);
typedef void (*ws_share_device_cb)(const char *peer_id, void *user_data);
typedef void (*ws_share_void_cb)(void *user_data);

typedef struct {
    ws_share_activate_cb on_ready;
    ws_share_device_cb on_device_clicked;
    ws_share_void_cb on_cancel_clicked;
} WsShareCallbacks;

typedef struct {
    void *user_data;
    WsShareCallbacks cb;
    AdwApplication *app;
    GtkWidget *window;
    GtkWidget *stack;
    GtkWidget *devices_group;
    GtkWidget *empty_lbl;
    GtkWidget *spinner;
    GtkWidget *file_summary_lbl;
    GtkWidget *tx_title_lbl;
    GtkWidget *tx_subtitle_lbl;
    GtkWidget *tx_progress;
    GtkWidget *tx_action_btn;
    int tx_finished;
    GPtrArray *device_rows;
} WsShareUi;

typedef struct {
    WsShareUi *ui;
    char *peer_id;
} DeviceRowCtx;

static void device_row_ctx_free(gpointer data, GClosure *closure) {
    (void)closure;
    DeviceRowCtx *ctx = (DeviceRowCtx *)data;
    g_free(ctx->peer_id);
    g_free(ctx);
}

static void on_device_row_activated(AdwActionRow *row, gpointer data) {
    (void)row;
    DeviceRowCtx *ctx = (DeviceRowCtx *)data;
    if (ctx->ui->cb.on_device_clicked) {
        ctx->ui->cb.on_device_clicked(ctx->peer_id, ctx->ui->user_data);
    }
}

static void on_tx_action_clicked(GtkButton *btn, gpointer data) {
    (void)btn;
    WsShareUi *ui = (WsShareUi *)data;
    if (ui->tx_finished) {
        gtk_window_close(GTK_WINDOW(ui->window));
    } else if (ui->cb.on_cancel_clicked) {
        ui->cb.on_cancel_clicked(ui->user_data);
    }
}

static void on_share_activate(GtkApplication *app, gpointer data) {
    WsShareUi *ui = (WsShareUi *)data;
    if (ui->window != NULL) {
        gtk_window_present(GTK_WINDOW(ui->window));
        return;
    }

    GtkWidget *win = adw_application_window_new(app);
    ui->window = win;
    gtk_window_set_title(GTK_WINDOW(win), "Share with Workspace");
    gtk_window_set_default_size(GTK_WINDOW(win), 450, 540);
    gtk_window_set_resizable(GTK_WINDOW(win), FALSE);

    GtkWidget *toolbar = adw_toolbar_view_new();
    GtkWidget *header = adw_header_bar_new();
    ui->spinner = adw_spinner_new();
    gtk_widget_set_margin_end(ui->spinner, 8);
    adw_header_bar_pack_end(ADW_HEADER_BAR(header), ui->spinner);
    adw_toolbar_view_add_top_bar(ADW_TOOLBAR_VIEW(toolbar), header);

    ui->stack = gtk_stack_new();
    gtk_stack_set_transition_type(GTK_STACK(ui->stack), GTK_STACK_TRANSITION_TYPE_SLIDE_LEFT_RIGHT);

    /* Page 1: Paired LAN Devices */
    GtkWidget *dev_page = gtk_box_new(GTK_ORIENTATION_VERTICAL, 16);
    gtk_widget_set_margin_top(dev_page, 18);
    gtk_widget_set_margin_bottom(dev_page, 18);
    gtk_widget_set_margin_start(dev_page, 20);
    gtk_widget_set_margin_end(dev_page, 20);

    ui->file_summary_lbl = gtk_label_new("");
    gtk_widget_add_css_class(ui->file_summary_lbl, "dim-label");
    gtk_widget_set_halign(ui->file_summary_lbl, GTK_ALIGN_START);
    gtk_box_append(GTK_BOX(dev_page), ui->file_summary_lbl);

    GtkWidget *scroll = gtk_scrolled_window_new();
    gtk_scrolled_window_set_policy(GTK_SCROLLED_WINDOW(scroll), GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
    gtk_widget_set_vexpand(scroll, TRUE);

    GtkWidget *group_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 12);
    ui->devices_group = adw_preferences_group_new();
    adw_preferences_group_set_title(ADW_PREFERENCES_GROUP(ui->devices_group), "Paired LAN Devices");
    gtk_box_append(GTK_BOX(group_box), ui->devices_group);

    ui->empty_lbl = gtk_label_new("正在搜索同一局域网下的已配对设备...\n请确保 寰宇工作台 后台及手机处于同一 Wi-Fi");
    gtk_label_set_justify(GTK_LABEL(ui->empty_lbl), GTK_JUSTIFY_CENTER);
    gtk_widget_add_css_class(ui->empty_lbl, "dim-label");
    gtk_widget_set_margin_top(ui->empty_lbl, 24);
    gtk_box_append(GTK_BOX(group_box), ui->empty_lbl);

    gtk_scrolled_window_set_child(GTK_SCROLLED_WINDOW(scroll), group_box);
    gtk_box_append(GTK_BOX(dev_page), scroll);
    gtk_stack_add_named(GTK_STACK(ui->stack), dev_page, "devices");

    /* Page 2: Transfer Progress */
    GtkWidget *tx_page = gtk_box_new(GTK_ORIENTATION_VERTICAL, 18);
    gtk_widget_set_valign(tx_page, GTK_ALIGN_CENTER);
    gtk_widget_set_margin_start(tx_page, 32);
    gtk_widget_set_margin_end(tx_page, 32);

    GtkWidget *tx_icon = gtk_image_new_from_icon_name("document-send-symbolic");
    gtk_image_set_pixel_size(GTK_IMAGE(tx_icon), 56);

    ui->tx_title_lbl = gtk_label_new("正在通过局域网发送...");
    gtk_widget_add_css_class(ui->tx_title_lbl, "title-2");
    gtk_label_set_wrap(GTK_LABEL(ui->tx_title_lbl), TRUE);

    ui->tx_subtitle_lbl = gtk_label_new("");
    gtk_widget_add_css_class(ui->tx_subtitle_lbl, "dim-label");
    gtk_label_set_wrap(GTK_LABEL(ui->tx_subtitle_lbl), TRUE);

    ui->tx_progress = gtk_progress_bar_new();
    gtk_progress_bar_set_show_text(GTK_PROGRESS_BAR(ui->tx_progress), TRUE);

    ui->tx_action_btn = gtk_button_new_with_label("Cancel");
    gtk_widget_add_css_class(ui->tx_action_btn, "pill");
    gtk_widget_set_halign(ui->tx_action_btn, GTK_ALIGN_CENTER);
    g_signal_connect(ui->tx_action_btn, "clicked", G_CALLBACK(on_tx_action_clicked), ui);

    gtk_box_append(GTK_BOX(tx_page), tx_icon);
    gtk_box_append(GTK_BOX(tx_page), ui->tx_title_lbl);
    gtk_box_append(GTK_BOX(tx_page), ui->tx_progress);
    gtk_box_append(GTK_BOX(tx_page), ui->tx_subtitle_lbl);
    gtk_box_append(GTK_BOX(tx_page), ui->tx_action_btn);
    gtk_stack_add_named(GTK_STACK(ui->stack), tx_page, "transfer");

    adw_toolbar_view_set_content(ADW_TOOLBAR_VIEW(toolbar), ui->stack);
    adw_application_window_set_content(ADW_APPLICATION_WINDOW(win), toolbar);

    gtk_window_present(GTK_WINDOW(win));

    if (ui->cb.on_ready) {
        ui->cb.on_ready(ui->user_data);
    }
}

WsShareUi *ws_share_ui_new(WsShareCallbacks cb, void *user_data) {
    WsShareUi *ui = g_new0(WsShareUi, 1);
    ui->cb = cb;
    ui->user_data = user_data;
    ui->device_rows = g_ptr_array_new();
    ui->app = adw_application_new("me.edfork.workspace.share", G_APPLICATION_NON_UNIQUE);
    g_signal_connect(ui->app, "activate", G_CALLBACK(on_share_activate), ui);
    return ui;
}

int ws_share_ui_run(WsShareUi *ui) {
    char *argv[] = { (char *)"workspace-share", NULL };
    int rc = g_application_run(G_APPLICATION(ui->app), 1, argv);
    g_ptr_array_free(ui->device_rows, TRUE);
    g_object_unref(ui->app);
    g_free(ui);
    return rc;
}

void ws_share_set_file_summary(WsShareUi *ui, const char *summary) {
    if (ui && ui->file_summary_lbl) {
        gtk_label_set_text(GTK_LABEL(ui->file_summary_lbl), summary);
    }
}

void ws_share_set_empty_message(WsShareUi *ui, const char *msg) {
    if (ui && ui->empty_lbl) {
        gtk_label_set_text(GTK_LABEL(ui->empty_lbl), msg);
    }
}

void ws_share_clear_devices(WsShareUi *ui) {
    if (!ui || !ui->devices_group) return;
    for (guint i = 0; i < ui->device_rows->len; i++) {
        GtkWidget *row = g_ptr_array_index(ui->device_rows, i);
        adw_preferences_group_remove(ADW_PREFERENCES_GROUP(ui->devices_group), row);
    }
    g_ptr_array_set_size(ui->device_rows, 0);
    gtk_widget_set_visible(ui->empty_lbl, TRUE);
}

void ws_share_add_device(WsShareUi *ui, const char *peer_id, const char *name, const char *subtitle) {
    if (!ui || !ui->devices_group) return;
    gtk_widget_set_visible(ui->empty_lbl, FALSE);

    GtkWidget *row = adw_action_row_new();
    adw_preferences_row_set_title(ADW_PREFERENCES_ROW(row), name);
    adw_action_row_set_subtitle(ADW_ACTION_ROW(row), subtitle);
    gtk_list_box_row_set_activatable(GTK_LIST_BOX_ROW(row), TRUE);

    GtkWidget *icon = gtk_image_new_from_icon_name("phone-symbolic");
    adw_action_row_add_prefix(ADW_ACTION_ROW(row), icon);
    GtkWidget *chevron = gtk_image_new_from_icon_name("go-next-symbolic");
    adw_action_row_add_suffix(ADW_ACTION_ROW(row), chevron);

    DeviceRowCtx *ctx = g_new0(DeviceRowCtx, 1);
    ctx->ui = ui;
    ctx->peer_id = g_strdup(peer_id);
    g_signal_connect_data(row, "activated", G_CALLBACK(on_device_row_activated), ctx, device_row_ctx_free, 0);

    adw_preferences_group_add(ADW_PREFERENCES_GROUP(ui->devices_group), row);
    g_ptr_array_add(ui->device_rows, row);
}

void ws_share_show_transfer(WsShareUi *ui, const char *title, const char *subtitle,
                            double fraction, int finished, int error) {
    if (!ui || !ui->stack) return;
    gtk_widget_set_visible(ui->spinner, FALSE);
    gtk_stack_set_visible_child_name(GTK_STACK(ui->stack), "transfer");
    gtk_label_set_text(GTK_LABEL(ui->tx_title_lbl), title);
    gtk_label_set_text(GTK_LABEL(ui->tx_subtitle_lbl), subtitle);
    gtk_progress_bar_set_fraction(GTK_PROGRESS_BAR(ui->tx_progress), fraction);

    ui->tx_finished = finished;
    if (finished) {
        gtk_button_set_label(GTK_BUTTON(ui->tx_action_btn), error ? "Close" : "Done");
        gtk_widget_remove_css_class(ui->tx_action_btn, "destructive-action");
        gtk_widget_add_css_class(ui->tx_action_btn, "suggested-action");
    } else {
        gtk_button_set_label(GTK_BUTTON(ui->tx_action_btn), "Cancel");
        gtk_widget_remove_css_class(ui->tx_action_btn, "suggested-action");
        gtk_widget_add_css_class(ui->tx_action_btn, "destructive-action");
    }
}

void ws_share_schedule_idle(void (*fn)(void *), void *arg) {
    g_idle_add_once((GSourceOnceFunc)fn, arg);
}
