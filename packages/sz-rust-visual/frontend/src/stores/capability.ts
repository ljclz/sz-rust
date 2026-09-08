import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@/api/tauri";

export interface CapabilityInfo {
  name: string;
  description: string;
  tags: string[];
  source: string;
  version: string;
  requiresConfirmation: boolean;
}

export const useCapabilityStore = defineStore("capability", () => {
  const capabilities = ref<CapabilityInfo[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);

  async function list() {
    loading.value = true;
    error.value = null;
    try {
      capabilities.value = await invoke<CapabilityInfo[]>("cap_list");
    } catch (e) {
      error.value = String(e);
    } finally {
      loading.value = false;
    }
  }

  async function call(name: string, args: Record<string, unknown>): Promise<unknown> {
    return invoke("cap_call", { name, args });
  }

  return { capabilities, loading, error, list, call };
});