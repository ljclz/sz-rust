<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useCapabilityStore } from "@/stores/capability";
import { usePluginStore } from "@/stores/plugin";

const capStore = useCapabilityStore();
const pluginStore = usePluginStore();
const selectedCap = ref<string | null>(null);
const capArgs = ref("{}");
const capResult = ref<string>("");

onMounted(async () => {
  await capStore.list();
  await pluginStore.list();
});

async function handleCall() {
  if (!selectedCap.value) return;
  try {
    const args = JSON.parse(capArgs.value);
    const result = await capStore.call(selectedCap.value, args);
    capResult.value = JSON.stringify(result, null, 2);
  } catch (e) {
    capResult.value = String(e);
  }
}
</script>

<template>
  <div class="plugin-panel">
    <h2>{{ $t("panel.plugin") }}</h2>
    <section class="capabilities">
      <h3>{{ $t("capability.title") }}</h3>
      <div v-if="capStore.loading">加载中...</div>
      <div v-else-if="capStore.error" class="error">{{ capStore.error }}</div>
      <ul v-else>
        <li
          v-for="cap in capStore.capabilities"
          :key="cap.name"
          :class="{ selected: selectedCap === cap.name }"
          @click="selectedCap = cap.name"
        >
          {{ cap.name }} — {{ cap.description }}
        </li>
      </ul>
      <div v-if="selectedCap" class="cap-call">
        <label>{{ $t("capability.args") }}</label>
        <textarea v-model="capArgs" rows="4" />
        <button @click="handleCall">{{ $t("capability.call") }}</button>
        <pre v-if="capResult" class="result">{{ capResult }}</pre>
      </div>
    </section>
    <section class="plugins">
      <h3>{{ $t("panel.plugin") }}</h3>
      <div v-for="p in pluginStore.plugins" :key="p.name" class="plugin-row">
        <span>{{ p.name }}</span>
        <span class="version">{{ p.version }}</span>
        <button
          :class="{ enabled: p.enabled }"
          @click="pluginStore.toggle(p.name, !p.enabled)"
        >
          {{ p.enabled ? $t("plugin.disable") : $t("plugin.enable") }}
        </button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.plugin-panel {
  padding: 20px;
}
section {
  margin-bottom: 24px;
}
ul {
  list-style: none;
  padding: 0;
}
li {
  padding: 6px 8px;
  cursor: pointer;
  border-radius: 4px;
  font-size: 13px;
}
li:hover {
  background: #2d2d2d;
}
li.selected {
  background: #09977e;
  color: #fff;
}
.cap-call {
  margin-top: 12px;
}
textarea {
  width: 100%;
  padding: 8px;
  background: #2d2d2d;
  border: 1px solid #3c3c3c;
  color: #d4d4d4;
  border-radius: 4px;
  font-size: 12px;
  font-family: monospace;
}
button {
  padding: 6px 12px;
  background: #09977e;
  border: none;
  color: #fff;
  border-radius: 4px;
  cursor: pointer;
  font-size: 12px;
}
.result {
  margin-top: 8px;
  padding: 8px;
  background: #1e1e1e;
  border-radius: 4px;
  font-size: 12px;
  white-space: pre-wrap;
}
.plugin-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 6px 0;
  font-size: 13px;
}
.version {
  color: #888;
  font-size: 12px;
}
.enabled {
  background: #f48771;
}
.error {
  color: #f48771;
}
</style>