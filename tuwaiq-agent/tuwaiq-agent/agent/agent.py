"""Agent core: the orchestration layer between a ModelProvider and the
BrokerClient.

Security-relevant invariant this file exists to enforce: **this file never
calls subprocess, os.system, os.exec*, or anything else that touches the OS
directly.** The only way anything in this process can affect the outside
world is `self._broker.call(tool, arguments)`, and `tool` is checked against
`KNOWN_TOOLS` before that call is ever made -- so even if a future, real
model provider hallucinated an arbitrary tool name or a shell-command-shaped
string, it is rejected here, in Python, before it would even reach the
broker (which independently re-validates it again on the Rust side -- two
layers, not one, is deliberate; see architecture.md's "Defense in depth").
"""

from __future__ import annotations

import logging

from broker_client import BrokerClient, BrokerUnavailableError
from model_provider import AgentAction, ModelProvider
from protocol import KNOWN_TOOLS

logger = logging.getLogger("tuwaiq_agent.agent")


class Agent:
    def __init__(self, model: ModelProvider, broker: BrokerClient):
        self._model = model
        self._broker = broker

    def handle(self, user_message: str) -> str:
        try:
            action: AgentAction = self._model.decide(user_message)
        except Exception:
            # A crash inside the model/provider layer must not crash the
            # whole agent process (requirement 15's "AI process crash" case,
            # tested directly in tests/) -- caught here, at the outermost
            # orchestration boundary, and turned into a safe user-facing
            # message instead of propagating.
            logger.exception("model provider raised while deciding an action")
            return "Sorry, I ran into a problem understanding that. Could you try again?"

        if action.kind == "respond":
            return action.text or ""

        return self._handle_tool_call(user_message, action)

    def _handle_tool_call(self, user_message: str, action: AgentAction) -> str:
        tool = action.tool or ""

        # First enforcement layer: reject anything not in the known,
        # fixed tool set before it ever reaches the broker process at all.
        if tool not in KNOWN_TOOLS:
            logger.warning("model requested unknown tool %r; refusing to call broker", tool)
            return "I don't have a way to do that yet."

        try:
            response = self._broker.call(tool, action.arguments)
        except BrokerUnavailableError as e:
            logger.error("broker unavailable: %s", e)
            return "System tools are temporarily unavailable. Please try again shortly."

        if response.ok:
            try:
                return self._model.explain(user_message, tool, response.result or {})
            except Exception:
                logger.exception("model provider raised while explaining a result")
                return f"I got a result but had trouble explaining it: {response.result}"

        try:
            return self._model.explain_error(
                user_message, tool, response.error_code or "internal_error", response.error_message or ""
            )
        except Exception:
            logger.exception("model provider raised while explaining an error")
            return "I couldn't complete that request."
