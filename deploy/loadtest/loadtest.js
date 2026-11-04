import http from 'k6/http';
import { check, sleep, group } from 'k6';
import { Rate, Trend, Counter } from 'k6/metrics';

// 自定义指标
const errorRate = new Rate('errors');
const latencyTrend = new Trend('api_latency');
const requestCount = new Counter('total_requests');

// 压测目标配置
const BASE_URL = __ENV.TARGET_URL || 'http://localhost:8300';

export const options = {
  // 阶梯式加压：模拟真实流量波动
  stages: [
    { duration: '1m', target: 50 },   // 热身期：50 QPS
    { duration: '3m', target: 200 },  // 加压期：200 QPS
    { duration: '5m', target: 500 },  // 峰值期：500 QPS
    { duration: '2m', target: 1000 }, // 极限期：1000 QPS
    { duration: '2m', target: 500 },  // 回落期
    { duration: '2m', target: 100 },  // 恢复期
  ],
  thresholds: {
    http_req_duration: ['p(99)<2000', 'p(95)<1000', 'avg<500'],
    http_req_failed: ['rate<0.05'],
    errors: ['rate<0.05'],
  },
};

export default function () {
  group('健康检查', () => {
    const res = http.get(`${BASE_URL}/health`);
    check(res, {
      '健康检查通过': (r) => r.status === 200,
    });
    latencyTrend.add(res.timings.duration);
    errorRate.add(res.status >= 500);
    requestCount.add(1);
    sleep(0.1);
  });

  group('API 请求', () => {
    // GET 请求（模拟商户查询）
    const res = http.get(`${BASE_URL}/api/v1/merchant/1`, {
      headers: { 'Authorization': 'Bearer test-token' },
    });
    check(res, {
      'API 响应正常': (r) => r.status < 500,
      'API 响应 < 2s': (r) => r.timings.duration < 2000,
    });
    latencyTrend.add(res.timings.duration);
    errorRate.add(res.status >= 500);
    requestCount.add(1);
    sleep(0.5);
  });

  group('POST 请求', () => {
    const payload = JSON.stringify({ name: 'test', amount: 100 });
    const res = http.post(`${BASE_URL}/api/v1/order`, payload, {
      headers: {
        'Content-Type': 'application/json',
        'Authorization': 'Bearer test-token',
      },
    });
    check(res, {
      'POST 请求成功': (r) => r.status < 500,
    });
    latencyTrend.add(res.timings.duration);
    errorRate.add(res.status >= 500);
    requestCount.add(1);
    sleep(1.0);
  });
}
