"""最小烟测：health 端点。"""
from fastapi.testclient import TestClient

from qr_service.main import app


def test_health() -> None:
    client = TestClient(app)
    resp = client.get("/health")
    assert resp.status_code == 200
    body = resp.json()
    assert body["status"] == "ok"
    assert "version" in body
