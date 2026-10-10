#!/usr/bin/env python3
"""Exercise Acteon with the pinned official A2A Python SDK."""

from __future__ import annotations

import argparse
import asyncio

import httpx
from a2a.client import ClientConfig, create_client
from a2a.helpers import new_text_message
from a2a.types import (
    CancelTaskRequest,
    GetTaskRequest,
    ListTasksRequest,
    Role,
    SendMessageRequest,
    TaskState,
)


async def run(base_url: str, token: str) -> None:
    headers = {"Authorization": f"Bearer {token}"}
    async with httpx.AsyncClient(headers=headers, timeout=15.0) as http:
        # Acteon's generic task endpoint is asynchronous. Polling mode makes
        # the SDK set SendMessageConfiguration.returnImmediately, matching
        # the server's durable acceptance response.
        config = ClientConfig(streaming=False, polling=True, httpx_client=http)
        client = await create_client(base_url, client_config=config)
        try:
            request = SendMessageRequest(
                message=new_text_message(
                    "inspect the governed city",
                    role=Role.ROLE_USER,
                )
            )
            responses = [item async for item in client.send_message(request)]
            assert len(responses) == 1
            assert responses[0].HasField("task")
            submitted = responses[0].task
            assert submitted.status.state == TaskState.TASK_STATE_SUBMITTED
            assert submitted.history[0].role == Role.ROLE_USER

            observed = await client.get_task(GetTaskRequest(id=submitted.id))
            assert observed.id == submitted.id
            assert observed.status.state == TaskState.TASK_STATE_SUBMITTED

            listed = await client.list_tasks(ListTasksRequest(page_size=10))
            assert listed.page_size == 10
            assert listed.total_size >= 1
            assert submitted.id in {task.id for task in listed.tasks}

            canceled = await client.cancel_task(CancelTaskRequest(id=submitted.id))
            assert canceled.id == submitted.id
            assert canceled.status.state == TaskState.TASK_STATE_CANCELED

            replayed = await client.get_task(GetTaskRequest(id=submitted.id))
            assert replayed.status.state == TaskState.TASK_STATE_CANCELED
            print(
                "official a2a-sdk conformance: "
                f"task={submitted.id} send/get/list/cancel/get passed"
            )
        finally:
            await client.close()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--token", required=True)
    args = parser.parse_args()
    asyncio.run(run(args.base_url, args.token))


if __name__ == "__main__":
    main()
