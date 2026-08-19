use serde::{
    Deserialize,
    Serialize,
};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionLinks {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ticket_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spec_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub doc_evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_run_id: Option<String>,
}

impl SessionLinks {
    pub fn links_to_ticket(
        &self,
        ticket_id: &str,
    ) -> bool {
        self.ticket_ids.iter().any(|id| id == ticket_id)
    }

    pub fn links_to_spec(
        &self,
        spec_id: &str,
    ) -> bool {
        self.spec_ids.iter().any(|id| id == spec_id)
    }
}
