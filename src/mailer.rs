use crate::config::{NotificationConfig, SmtpConfig};
use crate::monitor::{ChangeType, FileChange};
use lettre::{
    message::Mailbox,
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
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

    /// Build alert email subject and body from a list of changes.
    fn build_alert_body(&self, changes: &[FileChange]) -> (String, String) {
        let count = changes.len();
        let subject = format!(
            "{} 文件完整性告警 - {} 个文件发生变化",
            self.notification.subject_prefix, count
        );

        let mut body = String::new();
        for change in changes {
            body.push_str(&format!("文件: {}\n", change.path.display()));
            match change.change_type {
                ChangeType::Modified => body.push_str("状态: 已修改\n"),
                ChangeType::Deleted => body.push_str("状态: 已删除\n"),
            }
            body.push_str(&format!("旧哈希: {}\n", change.old_hash));
            body.push_str(&format!("新哈希: {}\n", change.new_hash));
            body.push_str(&format!("时间: {}\n\n", change.timestamp.format("%Y-%m-%d %H:%M:%S UTC")));
        }
        (subject, body)
    }

    fn build_startup_body(&self, file_hashes: &[(PathBuf, String)]) -> (String, String) {
        let subject = format!(
            "{} 监控已启动 - {} 个文件",
            self.notification.subject_prefix,
            file_hashes.len()
        );
        let mut body = String::from("文件清单:\n");
        for (path, hash) in file_hashes {
            body.push_str(&format!("  - {} (SHA256: {})\n", path.display(), hash));
        }
        (subject, body)
    }

    pub async fn send_alert(&self, changes: &[FileChange]) -> Result<(), String> {
        if changes.is_empty() {
            return Ok(());
        }
        let (subject, body) = self.build_alert_body(changes);
        self.send_email(&subject, &body).await
    }

    pub async fn send_startup_report(
        &self,
        file_hashes: &[(PathBuf, String)],
    ) -> Result<(), String> {
        let (subject, body) = self.build_startup_body(file_hashes);
        self.send_email(&subject, &body).await
    }

    async fn send_email(&self, subject: &str, body: &str) -> Result<(), String> {
        let from: Mailbox = format!("{} <{}>", self.smtp.from_name, self.smtp.username)
            .parse()
            .map_err(|e| format!("无效的发件人地址: {}", e))?;

        let to_addresses: Vec<Mailbox> = self
            .notification
            .to
            .iter()
            .filter_map(|addr| addr.parse().ok())
            .collect();

        if to_addresses.is_empty() {
            return Err("没有有效的收件人地址".to_string());
        }

        let mut email_builder = Message::builder()
            .from(from.clone())
            .subject(subject);

        for to in &to_addresses {
            email_builder = email_builder.to(to.clone());
        }

        let email = email_builder
            .body(body.to_string())
            .map_err(|e| format!("邮件构建失败: {}", e))?;

        let creds = Credentials::new(
            self.smtp.username.clone(),
            self.smtp.auth_code.clone(),
        );

        let mailer: AsyncSmtpTransport<Tokio1Executor> =
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.smtp.host)
                .port(self.smtp.port)
                .credentials(creds)
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
    fn test_alert_body_contains_file_info() {
        let mailer = make_mailer();
        let changes = vec![FileChange {
            path: PathBuf::from("/etc/hosts"),
            change_type: ChangeType::Modified,
            old_hash: "aaa".into(),
            new_hash: "bbb".into(),
            timestamp: Utc::now(),
        }];
        let (subject, body) = mailer.build_alert_body(&changes);
        assert!(subject.contains("[Test]"));
        assert!(subject.contains("1 个文件"));
        assert!(body.contains("/etc/hosts"));
        assert!(body.contains("已修改"));
        assert!(body.contains("aaa"));
        assert!(body.contains("bbb"));
    }

    #[test]
    fn test_alert_body_deleted_file() {
        let mailer = make_mailer();
        let changes = vec![FileChange {
            path: PathBuf::from("/tmp/gone.txt"),
            change_type: ChangeType::Deleted,
            old_hash: "oldhash".into(),
            new_hash: "<deleted>".into(),
            timestamp: Utc::now(),
        }];
        let (_, body) = mailer.build_alert_body(&changes);
        assert!(body.contains("已删除"));
    }

    #[test]
    fn test_startup_report_contains_file_list() {
        let mailer = make_mailer();
        let files = vec![
            (PathBuf::from("/etc/a"), "hashA".into()),
            (PathBuf::from("/etc/b"), "hashB".into()),
        ];
        let (subject, body) = mailer.build_startup_body(&files);
        assert!(subject.contains("监控已启动"));
        assert!(subject.contains("2 个文件"));
        assert!(body.contains("/etc/a"));
        assert!(body.contains("hashA"));
        assert!(body.contains("/etc/b"));
        assert!(body.contains("hashB"));
    }
}
