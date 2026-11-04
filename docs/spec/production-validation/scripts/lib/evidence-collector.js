import fs from 'fs';
import path from 'path';

export class EvidenceCollector {
    constructor(projectRoot) {
        this.projectRoot = projectRoot;
    }

    createEvidence(conclusion, file, line) {
        return {
            conclusion,
            file,
            line,
            verified: false,
        };
    }

    async verifyEvidence(evidence) {
        const filePath = path.isAbsolute(evidence.file)
            ? evidence.file
            : path.resolve(this.projectRoot, evidence.file);

        if (!fs.existsSync(filePath)) {
            evidence.verified = false;
            evidence.verifyError = `文件不存在: ${filePath}`;
            return evidence;
        }

        const content = fs.readFileSync(filePath, 'utf-8');
        const lines = content.split('\n');

        const lineSpec = evidence.line;
        let startLine, endLine;

        if (typeof lineSpec === 'string' && lineSpec.includes('-')) {
            const [start, end] = lineSpec.split('-').map(s => parseInt(s.trim(), 10));
            startLine = start;
            endLine = end;
        } else {
            startLine = endLine = parseInt(lineSpec, 10);
        }

        if (isNaN(startLine) || isNaN(endLine) || startLine < 1 || endLine < 1) {
            evidence.verified = false;
            evidence.verifyError = `无效行号: ${lineSpec}`;
            return evidence;
        }

        if (endLine > lines.length) {
            evidence.verified = false;
            evidence.verifyError = `行号超出文件范围: ${endLine} > ${lines.length}`;
            return evidence;
        }

        let hasContent = false;
        for (let i = startLine - 1; i < endLine; i++) {
            if (lines[i] && lines[i].trim().length > 0) {
                hasContent = true;
                break;
            }
        }

        evidence.verified = hasContent;
        if (!hasContent) {
            evidence.verifyError = `行 ${startLine}-${endLine} 无内容`;
        }

        return evidence;
    }

    async verifyAll(evidences) {
        const failed = [];

        for (const evidence of evidences) {
            await this.verifyEvidence(evidence);
            if (!evidence.verified) {
                failed.push(evidence);
            }
        }

        return {
            total: evidences.length,
            passed: evidences.length - failed.length,
            failed,
        };
    }
}