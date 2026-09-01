## 1. 上传新 prometheus.yml（含 bearer_token）
- 上传完成 ✅
## 2. 上传新启动脚本
- 上传完成 ✅
## 3. 重启 sz300-server
- 启动中...
- 进程: 3763161 ./target/release/sz300-server
- health: {"code":1,"msg":"success","data":{"status":"ok","version":"1.2.0","service":"sz300-server","timestamp":1788246842}}
## 4. 验证 metrics（bearer token）
- 无 token: HTTP 403（预期 403）
- 有 token: HTTP 200（预期 200）
## 5. 重启 Prometheus
time="2026-09-01T15:14:03+08:00" level=warning msg="/www/rust/sz-rust-new/deploy/docker-compose.yml: `version` is obsolete"
 Container prometheus  Restarting
 Container prometheus  Started
## 6. Prometheus healthy
- Prometheus Server is Healthy.
## 7. 等待 20s for scrape...
## 8. Prometheus targets
- http://cadvisor:8080/metrics | health=down | lastError=Get "http://cadvisor:8080/metrics": dial tcp: lookup cadvisor on 127.0.0.11:53: 
- http://host.docker.internal:8300/metrics | health=up | lastError=
## 9. 容器状态
grafana	Up 16 minutes	127.0.0.1:3000->3000/tcp
prometheus	Up 30 seconds	127.0.0.1:9090->9090/tcp
alertmanager	Up 16 minutes	127.0.0.1:9093->9093/tcp
## 10. 端口绑定
LISTEN 0      4096       127.0.0.1:9090       0.0.0.0:*    users:(("docker-proxy",pid=3763354,fd=4))                                                                                                                                                                                                                       
LISTEN 0      4096       127.0.0.1:9093       0.0.0.0:*    users:(("docker-proxy",pid=3758085,fd=4))                                                                                                                                                                                                                       
LISTEN 0      4096       127.0.0.1:3000       0.0.0.0:*    users:(("docker-proxy",pid=3758278,fd=4))
## 11. Grafana
- {
  "database": "ok",
  "version": "11.2.2",
  "commit": "dbf571744d7dd2276e3198b54330e5a561b83953"
}
## 12. AlertManager
- OK
## 13. 公网检查
- LISTEN 0      4096       127.0.0.1:9090       0.0.0.0:*    users:(("docker-proxy",pid=3763354,fd=4))                                                                                                                                                                                                                       
LISTEN 0      4096       127.0.0.1:9093       0.0.0.0:*    users:(("docker-proxy",pid=3758085,fd=4))                                                                                                                                                                                                                       
LISTEN 0      4096       127.0.0.1:3000       0.0.0.0:*    users:(("docker-proxy",pid=3758278,fd=4))