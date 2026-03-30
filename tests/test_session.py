"""Tests for claude/session.py — ported from Rust tests."""

from datetime import datetime, timedelta, timezone

from oac_slack_bot.claude.session import Session, SessionStore
from oac_slack_bot.claude.types import Message, Role


def test_session_key_dm():
    assert SessionStore.key("D01ABC123", None) == "dm-D01ABC123"

def test_session_key_thread():
    assert SessionStore.key("C01XYZ999", "1700000000.123456") == "C01XYZ999-1700000000.123456"

def test_session_key_channel_no_thread():
    assert SessionStore.key("C01XYZ999", None) == "ch-C01XYZ999"


def test_session_push_max_50():
    session = Session()
    for i in range(60):
        session.push(Message.user(f"message {i}"))

    assert len(session.messages) == 50
    msgs = session.messages_list()
    assert msgs[0].content == "message 10"
    assert msgs[49].content == "message 59"


def test_get_or_create_fresh():
    store = SessionStore(10, 1800)
    session = store.get_or_create("test-key")
    assert len(session.messages) == 0
    assert session.plugin_name is None

def test_get_or_create_returns_same():
    store = SessionStore(10, 1800)
    store.get_or_create("key1").push(Message.user("hello"))
    session = store.get_or_create("key1")
    assert len(session.messages) == 1


def test_cleanup_removes_stale():
    store = SessionStore(10, 1)  # 1-second TTL
    s = store.get_or_create("old-key")
    s.last_activity = datetime.now(timezone.utc) - timedelta(seconds=10)

    store.get_or_create("new-key").push(Message.user("hi"))

    store.cleanup()

    assert store.get("old-key") is None
    assert store.get("new-key") is not None


def test_role_serde():
    msg = Message.assistant("hello")
    assert msg.role == Role.ASSISTANT
