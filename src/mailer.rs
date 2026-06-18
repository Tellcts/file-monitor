use crate::config::{NotificationConfig, SmtpConfig};
use crate::monitor::{ChangeType, FileChange};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, MultiPart, SinglePart, header::ContentType},
    transport::smtp::authentication::Credentials,
    transport::smtp::client::{Tls, TlsParameters},
};
use std::path::PathBuf;

pub struct Mailer {
    smtp: SmtpConfig,
    notification: NotificationConfig,
}

impl Mailer {
    pub fn new(smtp: SmtpConfig, notification: NotificationConfig) -> Self {
        Self { smtp, notification }
    }

    /// Shared HTML template shell with inline CSS for email client compatibility.
    fn html_shell(title: &str, content: &str, logo_text: &str) -> String {
        format!(
            r#"<!DOCTYPE html>
<html lang="zh-CN">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width"></head>
<body style="margin:0;padding:0;background:#f4f5f7;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif">
<table width="100%" cellpadding="0" cellspacing="0" style="background:#f4f5f7;padding:24px 0">
<tr><td align="center">
<table width="600" cellpadding="0" cellspacing="0"
       style="background:#ffffff;border-radius:8px;overflow:hidden;box-shadow:0 2px 8px rgba(0,0,0,.06)">
  <!-- Header -->
  <tr><td style="background:linear-gradient(135deg,#1a1a2e,#16213e);padding:28px 32px">
    <table width="100%" cellpadding="0" cellspacing="0"><tr>
      <td style="color:#e0e0e0;font-size:13px;letter-spacing:1px">{logo_text}</td>
    </tr></table>
    <div style="color:#ffffff;font-size:22px;font-weight:600;margin-top:8px">{title}</div>
  </td></tr>
  <!-- Body -->
  <tr><td style="padding:32px">
    {content}
  </td></tr>
  <!-- Footer -->
  <tr><td style="border-top:1px solid #eee;padding:16px 32px;color:#999;font-size:12px;text-align:center">
    File Monitor &mdash; 文件完整性校验守护进程
  </td></tr>
</table>
</td></tr>
</table>
</body>
</html>"#,
            logo_text = logo_text,
            title = title,
            content = content
        )
    }

    /// Generate HTML for a status badge.
    fn status_badge(status: &str, color: &str, bg: &str) -> String {
        format!(
            r#"<span style="display:inline-block;padding:2px 10px;border-radius:12px;font-size:12px;font-weight:600;color:{};background:{}">{}</span>"#,
            color, bg, status
        )
    }

    /// Format a UTC timestamp as Beijing time (UTC+8) for email display.
    fn fmt_beijing(dt: &chrono::DateTime<chrono::Utc>) -> String {
        let offset = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        dt.with_timezone(&offset)
            .format("%Y-%m-%d %H:%M:%S 北京时间")
            .to_string()
    }

    /// Build a card row for a single file change.
    fn change_card(c: &FileChange) -> String {
        let (badge_html, icon) = match c.change_type {
            ChangeType::Modified => (Self::status_badge("已修改", "#b45309", "#fef3c7"), "⚠️"),
            ChangeType::Deleted => (Self::status_badge("已删除", "#dc2626", "#fee2e2"), "🗑"),
        };

        let hash_section = match c.change_type {
            ChangeType::Modified => format!(
                r#"<tr><td style="color:#999;font-size:12px;padding:4px 0 0 0">旧哈希</td>
                <td style="font-family:'SF Mono',Monaco,'Cascadia Code',monospace;font-size:12px;color:#b91c1c;padding:4px 0 0 8px">{}</td></tr>
                <tr><td style="color:#999;font-size:12px;padding:4px 0 0 0">新哈希</td>
                <td style="font-family:'SF Mono',Monaco,'Cascadia Code',monospace;font-size:12px;color:#15803d;padding:4px 0 0 8px">{}</td></tr>"#,
                c.old_hash, c.new_hash
            ),
            ChangeType::Deleted => format!(
                r#"<tr><td style="color:#999;font-size:12px;padding:4px 0 0 0">原哈希</td>
                <td style="font-family:'SF Mono',Monaco,'Cascadia Code',monospace;font-size:12px;color:#b91c1c;padding:4px 0 0 8px">{}</td></tr>"#,
                c.old_hash
            ),
        };

        format!(
            r#"<table width="100%" cellpadding="0" cellspacing="0"
       style="border:1px solid #e5e7eb;border-radius:6px;margin-bottom:16px">
  <tr><td style="padding:16px">
    <table width="100%" cellpadding="0" cellspacing="0">
      <tr>
        <td style="font-size:15px;font-weight:600;color:#1f2937;padding-bottom:8px">
          {icon} <span style="font-family:'SF Mono',Monaco,'Cascadia Code',monospace;font-size:13px">{path}</span>
        </td>
        <td align="right" style="padding-bottom:8px">{badge}</td>
      </tr>
      {hash_section}
      <tr><td colspan="2" style="color:#9ca3af;font-size:11px;padding-top:8px">{time}</td></tr>
    </table>
  </td></tr>
</table>"#,
            icon = icon,
            path = c.path.display(),
            badge = badge_html,
            hash_section = hash_section,
            time = Self::fmt_beijing(&c.timestamp),
        )
    }

    /// Build HTML alert email body.
    fn build_alert_html(&self, changes: &[FileChange]) -> (String, String) {
        let count = changes.len();
        let subject = format!(
            "{} 文件完整性告警 - {} 个文件发生变化",
            self.notification.subject_prefix, count
        );

        let cards: String = changes.iter().map(Self::change_card).collect();

        let content = format!(
            r#"<div style="color:#374151;font-size:14px;line-height:1.6;margin-bottom:20px">
  检测到 <strong style="color:#dc2626">{count} 个文件</strong> 发生变化：
</div>
{cards}"#,
            count = count,
            cards = cards
        );

        let html = Self::html_shell(
            &format!("文件完整性告警 — {} 个变化", count),
            &content,
            "⚠ ALERT",
        );

        (subject, html)
    }

    /// Build HTML startup report body.
    fn build_startup_html(&self, file_hashes: &[(PathBuf, String)]) -> (String, String) {
        let subject = format!(
            "{} 监控已启动 - {} 个文件",
            self.notification.subject_prefix,
            file_hashes.len()
        );

        let mut rows = String::new();
        for (path, hash) in file_hashes {
            rows.push_str(&format!(
                r#"<tr>
  <td style="font-family:'SF Mono',Monaco,'Cascadia Code',monospace;font-size:13px;color:#1f2937;padding:6px 0;border-bottom:1px solid #f3f4f6">{}</td>
  <td style="font-family:'SF Mono',Monaco,'Cascadia Code',monospace;font-size:11px;color:#6b7280;padding:6px 0;border-bottom:1px solid #f3f4f6;word-break:break-all">{}</td>
</tr>"#,
                path.display(),
                hash
            ));
        }

        let content = format!(
            r#"<div style="color:#374151;font-size:14px;line-height:1.6;margin-bottom:16px">
  监控已启动，正在监控 <strong>{count}</strong> 个文件：
</div>
<table width="100%" cellpadding="0" cellspacing="0" style="margin-top:8px">
  <tr style="background:#f9fafb">
    <td style="font-size:11px;color:#9ca3af;padding:6px 0;border-bottom:2px solid #e5e7eb;text-transform:uppercase;letter-spacing:.5px">文件路径</td>
    <td style="font-size:11px;color:#9ca3af;padding:6px 0;border-bottom:2px solid #e5e7eb;text-transform:uppercase;letter-spacing:.5px">SHA-256 哈希</td>
  </tr>
  {rows}
</table>"#,
            count = file_hashes.len(),
            rows = rows
        );

        let html = Self::html_shell("监控已启动", &content, "✓ ONLINE");

        (subject, html)
    }

    /// Generate plain text version from alert changes (for multipart fallback).
    fn build_alert_text(changes: &[FileChange]) -> String {
        let mut body = String::new();
        for change in changes {
            body.push_str(&format!("文件: {}\n", change.path.display()));
            match change.change_type {
                ChangeType::Modified => body.push_str("状态: 已修改\n"),
                ChangeType::Deleted => body.push_str("状态: 已删除\n"),
            }
            body.push_str(&format!("旧哈希: {}\n", change.old_hash));
            body.push_str(&format!("新哈希: {}\n", change.new_hash));
            body.push_str(&format!(
                "时间: {}\n\n",
                Self::fmt_beijing(&change.timestamp)
            ));
        }
        body
    }

    /// Generate plain text version from startup file list.
    fn build_startup_text(file_hashes: &[(PathBuf, String)]) -> String {
        let mut body = String::from("文件清单:\n");
        for (path, hash) in file_hashes {
            body.push_str(&format!("  - {} (SHA256: {})\n", path.display(), hash));
        }
        body
    }

    pub async fn send_alert(&self, changes: &[FileChange]) -> Result<(), String> {
        if changes.is_empty() {
            return Ok(());
        }
        let (subject, html) = self.build_alert_html(changes);
        let text = Self::build_alert_text(changes);
        self.send_email(&subject, &text, &html).await
    }

    pub async fn send_startup_report(
        &self,
        file_hashes: &[(PathBuf, String)],
    ) -> Result<(), String> {
        let (subject, html) = self.build_startup_html(file_hashes);
        let text = Self::build_startup_text(file_hashes);
        self.send_email(&subject, &text, &html).await
    }

    pub async fn send_shutdown_report(&self, file_count: usize) -> Result<(), String> {
        let now = Self::fmt_beijing(&chrono::Utc::now());
        let subject = format!("{} 监控已停止", self.notification.subject_prefix);

        let text = format!(
            "监控已停止。\n监控文件数: {}\n停止时间: {}\n",
            file_count, now
        );

        let html = Self::html_shell(
            "监控已停止",
            &format!(
                r#"<div style="color:#374151;font-size:14px;line-height:1.6">
  <p>文件完整性监控已<strong style="color:#dc2626">停止运行</strong>。</p>
  <table width="100%" cellpadding="0" cellspacing="0"
         style="border:1px solid #e5e7eb;border-radius:6px;margin-top:16px">
    <tr><td style="padding:12px 16px;border-bottom:1px solid #f3f4f6;color:#6b7280;font-size:13px">监控文件数</td>
        <td align="right" style="padding:12px 16px;border-bottom:1px solid #f3f4f6;font-weight:600;font-size:16px">{count}</td></tr>
    <tr><td style="padding:12px 16px;color:#6b7280;font-size:13px">停止时间</td>
        <td align="right" style="padding:12px 16px;font-size:13px;color:#1f2937">{time}</td></tr>
  </table>
</div>"#,
                count = file_count,
                time = now
            ),
            "✕ OFFLINE",
        );

        self.send_email(&subject, &text, &html).await
    }

    async fn send_email(
        &self,
        subject: &str,
        text_body: &str,
        html_body: &str,
    ) -> Result<(), String> {
        let from: Mailbox = format!("{} <{}>", self.smtp.from_name, self.smtp.username)
            .parse()
            .map_err(|e| format!("无效的发件人地址: {}", e))?;

        let to_addresses: Vec<Mailbox> = self
            .notification
            .to
            .iter()
            .map(|addr| addr.parse::<Mailbox>())
            .inspect(|r| {
                if let Err(e) = r {
                    log::warn!("无效的收件人地址: {}", e);
                }
            })
            .filter_map(|r| r.ok())
            .collect();

        if to_addresses.is_empty() {
            return Err("没有有效的收件人地址".to_string());
        }

        let mut email_builder = Message::builder().from(from.clone()).subject(subject);

        for to in &to_addresses {
            email_builder = email_builder.to(to.clone());
        }

        // Multipart with plain text fallback + HTML
        let email = email_builder
            .multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(text_body.to_string()),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(html_body.to_string()),
                    ),
            )
            .map_err(|e| format!("邮件构建失败: {}", e))?;

        let creds = Credentials::new(self.smtp.username.clone(), self.smtp.auth_code.clone());

        let tls_params = TlsParameters::builder(self.smtp.host.clone())
            .dangerous_accept_invalid_certs(true)
            .build()
            .map_err(|e| format!("TLS 参数配置失败: {}", e))?;

        let mailer: AsyncSmtpTransport<Tokio1Executor> =
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.smtp.host)
                .port(self.smtp.port)
                .credentials(creds)
                .tls(Tls::Wrapper(tls_params))
                .build();

        mailer
            .send(email)
            .await
            .map_err(|e| format!("SMTP 发送失败: {}", e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::ChangeType;
    use chrono::Utc;

    fn make_mailer() -> Mailer {
        Mailer::new(
            SmtpConfig {
                host: "smtp.test.com".into(),
                port: 465,
                username: "test@test.com".into(),
                auth_code: "secret".into(),
                from_name: "Test".into(),
            },
            NotificationConfig {
                to: vec!["admin@test.com".into()],
                subject_prefix: "[Test]".into(),
            },
        )
    }

    #[test]
    fn test_alert_html_contains_file_info() {
        let mailer = make_mailer();
        let changes = vec![FileChange {
            path: PathBuf::from("/etc/hosts"),
            change_type: ChangeType::Modified,
            old_hash: "aaa".into(),
            new_hash: "bbb".into(),
            timestamp: Utc::now(),
        }];
        let (subject, html) = mailer.build_alert_html(&changes);
        assert!(subject.contains("[Test]"));
        assert!(subject.contains("1 个文件"));
        assert!(html.contains("/etc/hosts"));
        assert!(html.contains("已修改"));
        assert!(html.contains("aaa"));
        assert!(html.contains("bbb"));
        // Verify HTML structure
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<html"));
    }

    #[test]
    fn test_alert_html_deleted_file() {
        let mailer = make_mailer();
        let changes = vec![FileChange {
            path: PathBuf::from("/tmp/gone.txt"),
            change_type: ChangeType::Deleted,
            old_hash: "oldhash".into(),
            new_hash: "<deleted>".into(),
            timestamp: Utc::now(),
        }];
        let (_, html) = mailer.build_alert_html(&changes);
        assert!(html.contains("已删除"));
        assert!(html.contains("oldhash"));
    }

    #[test]
    fn test_startup_html_contains_file_list() {
        let mailer = make_mailer();
        let files = vec![
            (PathBuf::from("/etc/a"), "hashA".into()),
            (PathBuf::from("/etc/b"), "hashB".into()),
        ];
        let (subject, html) = mailer.build_startup_html(&files);
        assert!(subject.contains("监控已启动"));
        assert!(subject.contains("2 个文件"));
        assert!(html.contains("/etc/a"));
        assert!(html.contains("hashA"));
        assert!(html.contains("/etc/b"));
        assert!(html.contains("hashB"));
        // Verify HTML structure
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn test_alert_text_contains_file_info() {
        let changes = vec![FileChange {
            path: PathBuf::from("/etc/hosts"),
            change_type: ChangeType::Modified,
            old_hash: "old".into(),
            new_hash: "new".into(),
            timestamp: Utc::now(),
        }];
        let text = Mailer::build_alert_text(&changes);
        assert!(text.contains("/etc/hosts"));
        assert!(text.contains("已修改"));
        assert!(text.contains("old"));
        assert!(text.contains("new"));
    }

    #[test]
    fn test_startup_text_contains_file_list() {
        let files = vec![(PathBuf::from("/etc/x"), "hashX".into())];
        let text = Mailer::build_startup_text(&files);
        assert!(text.contains("/etc/x"));
        assert!(text.contains("hashX"));
    }
}
