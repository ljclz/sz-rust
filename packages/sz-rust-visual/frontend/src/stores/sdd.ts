import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke, listen } from "@/api/tauri";

export interface SddSession {
    sessionId: string;
    feature: string;
    currentPhase: string;
    status: string;
    artifacts: Record<string, string>;
}

export interface PhaseEvent {
    sessionId: string;
    phase: string;
    eventType: string;
    message: string;
    timestamp: string;
}

export type SddPhase = "requirement" | "spec" | "design" | "task";
export type ReviewDecision = "approve" | "reject";

export const useSddStore = defineStore("sdd", () => {
    const session = ref<SddSession | null>(null);
    const events = ref<PhaseEvent[]>([]);
    const loading = ref(false);
    const error = ref<string | null>(null);
    let unlistenFn: (() => void) | null = null;

    async function start(feature: string, requirement: string, startPhase: SddPhase) {
        loading.value = true;
        error.value = null;
        try {
            session.value = await invoke<SddSession>("sdd_start", {
                feature,
                requirement,
                startPhase,
            });
            await subscribeEvents();
        } catch (e) {
            error.value = String(e);
        } finally {
            loading.value = false;
        }
    }

    async function submitReview(
        sessionId: string,
        phase: SddPhase,
        decision: ReviewDecision,
        comment?: string,
    ) {
        loading.value = true;
        try {
            session.value = await invoke<SddSession>("sdd_submit_review", {
                sessionId,
                phase,
                decision,
                comment: comment ?? null,
            });
        } catch (e) {
            error.value = String(e);
        } finally {
            loading.value = false;
        }
    }

    async function cancel(sessionId: string) {
        await invoke("sdd_cancel", { sessionId });
        session.value = null;
    }

    async function fetchStatus(sessionId: string) {
        session.value = await invoke<SddSession>("sdd_status", { sessionId });
    }

    async function readArtifact(sessionId: string, phase: SddPhase): Promise<string> {
        return invoke<string>("sdd_read_artifact", { sessionId, phase });
    }

    async function subscribeEvents() {
        if (unlistenFn) return;
        unlistenFn = await listen<PhaseEvent>("sdd_phase_event", (payload) => {
            events.value.push(payload);
        });
    }

    function unsubscribeEvents() {
        if (unlistenFn) {
            unlistenFn();
            unlistenFn = null;
        }
    }

    return {
        session,
        events,
        loading,
        error,
        start,
        submitReview,
        cancel,
        fetchStatus,
        readArtifact,
        subscribeEvents,
        unsubscribeEvents,
    };
});