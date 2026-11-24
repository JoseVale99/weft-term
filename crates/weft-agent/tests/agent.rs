use weft_agent::{Conversation, Decision, Guard, Proposal, Role};

#[test]
fn roles_use_provider_names() {
    assert_eq!(Role::System.as_str(), "system");
    assert_eq!(Role::User.as_str(), "user");
    assert_eq!(Role::Assistant.as_str(), "assistant");
    assert_eq!(Role::Tool.as_str(), "tool");
}

#[test]
fn a_conversation_keeps_the_system_prompt() {
    let mut conversation = Conversation::new("you are weft", 10);
    conversation.push(Role::User, "list files");
    conversation.push(Role::Assistant, "running ls");

    assert_eq!(conversation.len(), 3);
    assert_eq!(conversation.messages()[0].content, "you are weft");
    assert_eq!(conversation.last().unwrap().role, Role::Assistant);
    assert!(!conversation.is_empty());
}

#[test]
fn the_limit_evicts_the_oldest_non_system_message() {
    let mut conversation = Conversation::new("system", 2);
    conversation.push(Role::User, "one");
    conversation.push(Role::Assistant, "two");
    conversation.push(Role::User, "three");

    let contents: Vec<&str> = conversation
        .messages()
        .iter()
        .map(|message| message.content.as_str())
        .collect();
    assert_eq!(contents, ["system", "two", "three"]);
}

#[test]
fn an_empty_command_is_never_run() {
    let guard = Guard::new(true);
    let proposal = Proposal::new("   ", "nothing");
    assert_eq!(guard.evaluate(&proposal, Some(true)), Decision::Rejected);
}

#[test]
fn approval_is_required_by_default() {
    let guard = Guard::new(false);
    let proposal = Proposal::new("rm -rf build", "clean the build directory");

    assert_eq!(guard.evaluate(&proposal, None), Decision::NeedsApproval);
    assert_eq!(guard.evaluate(&proposal, Some(false)), Decision::Rejected);
    assert_eq!(guard.evaluate(&proposal, Some(true)), Decision::Run);
}

#[test]
fn auto_approve_runs_without_an_answer() {
    let guard = Guard::new(true);
    let proposal = Proposal::new("cargo test", "verify the change");
    assert_eq!(guard.evaluate(&proposal, None), Decision::Run);
}
