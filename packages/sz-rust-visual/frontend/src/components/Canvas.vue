<script setup lang="ts">
import RequirementPanel from "@/components/panels/RequirementPanel.vue";
import SpecPanel from "@/components/panels/SpecPanel.vue";
import DesignPanel from "@/components/panels/DesignPanel.vue";
import TaskBoard from "@/components/panels/TaskBoard.vue";
import LogPanel from "@/components/panels/LogPanel.vue";
import PreviewPanel from "@/components/panels/PreviewPanel.vue";
import PluginPanel from "@/components/panels/PluginPanel.vue";

defineProps<{ activePanel: string }>();
</script>

<template>
  <div class="canvas">
    <RequirementPanel v-show="activePanel === 'requirement'" />
    <SpecPanel v-show="activePanel === 'spec'" />
    <DesignPanel v-show="activePanel === 'design'" />
    <TaskBoard v-show="activePanel === 'tasks'" />
    <LogPanel v-show="activePanel === 'log'" />
    <PreviewPanel v-show="activePanel === 'preview'" />
    <PluginPanel v-show="activePanel === 'plugin'" />
  </div>
</template>

<style scoped>
.canvas {
  width: 100%;
  height: 100%;
  background: #252526;
  color: #d4d4d4;
  overflow: auto;
}
</style>