<script setup lang="ts">
import { ref, watch } from "vue";
import { useSddStore } from "@/stores/sdd";

const sdd = useSddStore();
const content = ref("");

watch(
  () => sdd.session?.artifacts?.spec,
  async () => {
    if (sdd.session) {
      try {
        content.value = await sdd.readArtifact(sdd.session.sessionId, "spec");
      } catch {
        content.value = "";
      }
    }
  },
  { immediate: true },
);
</script>

<template>
  <div class="spec-panel">
    <h2>{{ $t("panel.spec") }}</h2>
    <pre class="markdown-content">{{ content || "暂无 spec.md 产物" }}</pre>
  </div>
</template>

<style scoped>
.spec-panel {
  padding: 20px;
}
.markdown-content {
  white-space: pre-wrap;
  font-size: 13px;
  line-height: 1.6;
  color: #d4d4d4;
}
</style>