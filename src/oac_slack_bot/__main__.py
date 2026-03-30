"""Entry point: python -m oac_slack_bot."""

import asyncio

from oac_slack_bot.app import main

if __name__ == "__main__":
    asyncio.run(main())
