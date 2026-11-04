import { EventEmitter } from 'events';

export class HealthProbe extends EventEmitter {
    constructor(url, intervalMs = 5000, maxConsecutiveFailures = 3) {
        super();
        this.url = url;
        this.intervalMs = intervalMs;
        this.maxConsecutiveFailures = maxConsecutiveFailures;
        this.consecutiveFailures = 0;
        this.running = false;
        this.timer = null;
        this.history = [];
    }

    start() {
        this.running = true;
        this._tick();
    }

    async _tick() {
        while (this.running) {
            const status = await this._check();
            this.history.push({ time: new Date().toISOString(), status });
            this.emit('probe', status);

            if (status === 200) {
                this.consecutiveFailures = 0;
            } else {
                this.consecutiveFailures++;
                if (this.consecutiveFailures >= this.maxConsecutiveFailures) {
                    this.emit('unhealthy', { consecutiveFailures: this.consecutiveFailures, lastStatus: status, history: this.history });
                    this.stop();
                    return;
                }
            }
            await new Promise(r => { this.timer = setTimeout(r, this.intervalMs); });
        }
    }

    async _check() {
        try {
            const res = await fetch(this.url);
            return res.status;
        } catch {
            return 0;
        }
    }

    stop() {
        this.running = false;
        if (this.timer) clearTimeout(this.timer);
    }

    getHealthyRate() {
        if (this.history.length === 0) return 100;
        return (this.history.filter(h => h.status === 200).length / this.history.length) * 100;
    }
}