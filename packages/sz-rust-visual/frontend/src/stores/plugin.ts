import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@/api/tauri";

export interface PluginEntry {
    name: string;
    version: string;
    enabled: boolean;
    config: Record<string, unknown>;
}

export const usePluginStore = defineStore("plugin", () => {
    const plugins = ref<PluginEntry[]>([]);
    const loading = ref(false);
    const error = ref<string | null>(null);

    async function list() {
        loading.value = true;
        try {
            const raw = await invoke<PluginEntry[]>("plugin_list");
            plugins.value = raw;
        } catch (e) {
            error.value = String(e);
        } finally {
            loading.value = false;
        }
    }

    async function toggle(name: string, enabled: boolean) {
        await invoke("plugin_toggle", { name, enabled });
        const p = plugins.value.find((x) => x.name === name);
        if (p) p.enabled = enabled;
    }

    async function configure(name: string, config: Record<string, unknown>) {
        await invoke("plugin_configure", { name, config });
        const p = plugins.value.find((x) => x.name === name);
        if (p) p.config = config;
    }

    return { plugins, loading, error, list, toggle, configure };
});