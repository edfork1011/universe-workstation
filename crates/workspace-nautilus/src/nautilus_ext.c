/* SPDX-License-Identifier: GPL-3.0-or-later */
#include <nautilus-extension.h>
#include <glib.h>

extern void workspace_nautilus_on_activate(const char *const *paths, int count);

typedef struct {
    GObject parent_instance;
} WorkspaceNautilusExt;

typedef struct {
    GObjectClass parent_class;
} WorkspaceNautilusExtClass;

static GType provider_type = 0;

static void workspace_nautilus_menu_provider_iface_init(NautilusMenuProviderInterface *iface);

G_DEFINE_DYNAMIC_TYPE_EXTENDED(
    WorkspaceNautilusExt,
    workspace_nautilus_ext,
    G_TYPE_OBJECT,
    0,
    G_IMPLEMENT_INTERFACE_DYNAMIC(
        NAUTILUS_TYPE_MENU_PROVIDER,
        workspace_nautilus_menu_provider_iface_init
    )
)

static void workspace_nautilus_ext_init(WorkspaceNautilusExt *self) {
    (void)self;
}

static void workspace_nautilus_ext_class_init(WorkspaceNautilusExtClass *klass) {
    (void)klass;
}

static void workspace_nautilus_ext_class_finalize(WorkspaceNautilusExtClass *klass) {
    (void)klass;
}

static gpointer copy_ref_file(gconstpointer src, gpointer user_data) {
    (void)user_data;
    return g_object_ref((gpointer)src);
}

static void free_file_list(gpointer data, GClosure *closure) {
    (void)closure;
    g_list_free_full((GList *)data, g_object_unref);
}

static void on_share_menu_activate(NautilusMenuItem *item, gpointer user_data) {
    (void)item;
    GList *files = (GList *)user_data;
    GPtrArray *paths = g_ptr_array_new_with_free_func(g_free);

    for (GList *l = files; l != NULL; l = l->next) {
        NautilusFileInfo *fi = NAUTILUS_FILE_INFO(l->data);
        if (nautilus_file_info_is_directory(fi)) {
            continue;
        }
        GFile *gf = nautilus_file_info_get_location(fi);
        if (gf != NULL) {
            if (g_file_is_native(gf)) {
                char *p = g_file_get_path(gf);
                if (p != NULL && g_file_test(p, G_FILE_TEST_IS_REGULAR)) {
                    g_ptr_array_add(paths, p);
                } else {
                    g_free(p);
                }
            }
            g_object_unref(gf);
        }
    }

    if (paths->len > 0) {
        workspace_nautilus_on_activate((const char *const *)paths->pdata, (int)paths->len);
    }
    g_ptr_array_free(paths, TRUE);
}

static GList *get_file_items(NautilusMenuProvider *provider, GList *files) {
    (void)provider;
    if (files == NULL) {
        return NULL;
    }

    int regular_count = 0;
    for (GList *l = files; l != NULL; l = l->next) {
        NautilusFileInfo *fi = NAUTILUS_FILE_INFO(l->data);
        if (nautilus_file_info_is_directory(fi)) {
            continue;
        }
        char *scheme = nautilus_file_info_get_uri_scheme(fi);
        gboolean is_local = (g_strcmp0(scheme, "file") == 0);
        g_free(scheme);
        if (!is_local) {
            continue;
        }
        GFile *gf = nautilus_file_info_get_location(fi);
        if (gf != NULL) {
            char *p = g_file_get_path(gf);
            if (p != NULL && g_file_test(p, G_FILE_TEST_IS_REGULAR)) {
                regular_count++;
            }
            g_free(p);
            g_object_unref(gf);
        }
    }

    if (regular_count == 0) {
        return NULL;
    }

    NautilusMenuItem *item = nautilus_menu_item_new(
        "WorkspaceExtension::share",
        "Share with Workspace",
        "Send selected files to paired device over LAN",
        "document-send-symbolic"
    );

    GList *copied = g_list_copy_deep(files, copy_ref_file, NULL);
    g_signal_connect_data(
        item,
        "activate",
        G_CALLBACK(on_share_menu_activate),
        copied,
        free_file_list,
        0
    );

    return g_list_append(NULL, item);
}

static void workspace_nautilus_menu_provider_iface_init(NautilusMenuProviderInterface *iface) {
    iface->get_file_items = get_file_items;
}

void ws_nautilus_init(GTypeModule *module) {
    workspace_nautilus_ext_register_type(module);
    provider_type = workspace_nautilus_ext_get_type();
}

void ws_nautilus_shutdown(void) {
}

void ws_nautilus_list_types(const GType **types, int *num_types) {
    static GType type_list[1];
    type_list[0] = provider_type;
    *types = type_list;
    *num_types = 1;
}
