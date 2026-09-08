<script setup lang="ts">
import { ref, watch } from "vue";
import { useSddStore } from "@/stores/sdd";

const sdd = useSddStore();
const content = ref("");

watch(
  () => sdd.session?.artifacts?.design,
  async () => {
    if (sdd.session) {
      try {
        content.value = await sdd.readArtifact(sdd.session.sessionId, "design");
      } catch {
        content.value = "";
      }
    }
  },
  { immediate: true },
);
</script>

<template>
  <div class="design-panel">
    <h2>{{ $t("panel.design") }}</h2>
    <pre class="markdown-content">{{ content || "暂无 design.md 产物" }}</pre>
  </div>
</template>

<style scoped>
.design-panel {
  padding: 20px;
}
.markdown-content {
  white-space: pre-wrap;
  font-size: 13px;
  line-height: 1.6;
  color: #d4d4d4;
}
</style>