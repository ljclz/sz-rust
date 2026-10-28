<script setup lang="ts">
import { ref } from "vue";
import { useSddStore } from "@/stores/sdd";
import type { SddPhase, ReviewDecision } from "@/stores/sdd";

const sdd = useSddStore();
const feature = ref("");
const requirement = ref("");
const reviewComment = ref("");

async function handleStart() {
  if (!feature.value || !requirement.value) return;
  await sdd.start(feature.value, requirement.value, "requirement" as SddPhase);
}

async function handleReview(decision: ReviewDecision) {
  if (!sdd.session) return;
  await sdd.submitReview(
    sdd.session.sessionId,
    sdd.session.currentPhase as SddPhase,
    decision,
    reviewComment.value,
  );
  reviewComment.value = "";
}

async function handleCancel() {
  if (!sdd.session) return;
  await sdd.cancel(sdd.session.sessionId);
}
</script>

<template>
  <div class="requirement-panel">
    <h2>{{ $t("panel.requirement") }}</h2>
    <div class="form-group">
      <label>功能名称</label>
      <input v-model="feature" placeholder="例如：用户注册" />
    </div>
    <div class="form-group">
      <label>需求描述</label>
      <textarea v-model="requirement" rows="10" placeholder="输入需求描述..." />
    </div>
    <div class="actions">
      <button
        :disabled="sdd.loading || !feature || !requirement"
        @click="handleStart"
      >
        {{ $t("sdd.start") }}
      </button>
      <button
        v-if="sdd.session"
        :disabled="sdd.loading"
        @click="handleCancel"
      >
        {{ $t("sdd.cancel") }}
      </button>
    </div>
    <div v-if="sdd.error" class="error">{{ sdd.error }}</div>
    <div v-if="sdd.session" class="session-info">
      <p>会话: {{ sdd.session.sessionId }}</p>
      <p>阶段: {{ sdd.session.currentPhase }}</p>
      <p>状态: {{ sdd.session.status }}</p>
      <div v-if="sdd.session.status === 'awaiting_review'" class="review">
        <input v-model="reviewComment" :placeholder="$t('sdd.comment')" />
        <button @click="handleReview('approve')">{{ $t("sdd.approve") }}</button>
        <button @click="handleReview('reject')">{{ $t("sdd.reject") }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.requirement-panel {
  padding: 20px;
}
.form-group {
  margin-bottom: 16px;
}
label {
  display: block;
  margin-bottom: 4px;
  font-size: 13px;
}
input,
textarea {
  width: 100%;
  padding: 8px;
  background: #2d2d2d;
  border: 1px solid #3c3c3c;
  color: #d4d4d4;
  border-radius: 4px;
  font-size: 13px;
}
.actions {
  display: flex;
  gap: 8px;
}
button {
  padding: 8px 16px;
  background: #09977e;
  border: none;
  color: #fff;
  border-radius: 4px;
  cursor: pointer;
  font-size: 13px;
}
button:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.error {
  color: #f48771;
  margin-top: 8px;
}
.session-info {
  margin-top: 16px;
  padding: 12px;
  background: #2d2d2d;
  border-radius: 4px;
}
.review {
  display: flex;
  gap: 8px;
  margin-top: 8px;
}
</style>