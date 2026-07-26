{{/*
Expand the name of the chart.
*/}}
{{- define "szrsql.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/*
Create a default fully qualified app name.
We truncate at 63 chars because some Kubernetes name fields are limited to this
(by the DNS naming spec).
*/}}
{{- define "szrsql.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "szrsql.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/*
Common labels
*/}}
{{- define "szrsql.labels" -}}
helm.sh/chart: {{ include "szrsql.chart" . }}
{{ include "szrsql.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{/*
Selector labels
*/}}
{{- define "szrsql.selectorLabels" -}}
app.kubernetes.io/name: {{ include "szrsql.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{/*
ServiceAccount name
*/}}
{{- define "szrsql.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "szrsql.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{/*
Image reference: repository:tag
*/}}
{{- define "szrsql.image" -}}
{{- $repository := .Values.image.repository -}}
{{- $tag := .Values.image.tag | default .Chart.AppVersion -}}
{{- printf "%s:%s" $repository $tag -}}
{{- end -}}

{{/*
Build SzRSQL CLI args list
*/}}
{{- define "szrsql.args" -}}
- --host
- {{ .Values.args.host | quote }}
- --port
- {{ .Values.args.port | quote }}
- --server-version
- {{ .Values.args.server_version | quote }}
- --shutdown-timeout
- {{ .Values.args.shutdown_timeout | quote }}
- --crash-log-dir
- {{ .Values.args.crash_log_dir | quote }}
- --pid-file
- {{ .Values.args.pid_file | quote }}
{{- if .Values.args.no_backtrace }}
- --no-backtrace
{{- end }}
{{- if .Values.args.daemon }}
- --daemon
{{- end }}
{{- if .Values.http.enabled }}
- --http-port
- {{ .Values.http.port | quote }}
- --http-host
- {{ .Values.args.http_host | quote }}
{{- if .Values.http.auth_token }}
- --http-auth-token
- {{ .Values.http.auth_token | quote }}
{{- end }}
{{- end }}
{{- end -}}

{{/*
PVC name
*/}}
{{- define "szrsql.pvcName" -}}
{{- include "szrsql.fullname" . -}}-data
{{- end -}}
