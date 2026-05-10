pub(crate) struct HelpdeskDoc {
    pub(crate) id: &'static str,
    pub(crate) title: &'static str,
    pub(crate) content: &'static str,
}

pub(crate) fn helpdesk_docs() -> Vec<HelpdeskDoc> {
    vec![
        HelpdeskDoc {
            id: "kb-password-reset",
            title: "Reset your password",
            content: "Users can reset a password from the sign-in page by selecting Forgot password, entering the account email, and following the reset link. Reset links expire after 30 minutes.",
        },
        HelpdeskDoc {
            id: "kb-lost-email-access",
            title: "Account recovery when email is unavailable",
            content: "If a user no longer has access to the account email, support must verify identity with the last invoice id and the last four digits of the payment method before changing the email address.",
        },
        HelpdeskDoc {
            id: "kb-billing-upgrade",
            title: "Billing after subscription upgrade",
            content: "After an upgrade, a prorated charge may appear immediately. Duplicate charges should be escalated to billing support with invoice ids.",
        },
    ]
}
