.PHONY: build run-dev run-dev-no-autoreload

build:
	@if [ -x /usr/bin/distrobox-enter ]; then \
		/usr/bin/distrobox-enter -n apps-env -- cargo build --release; \
	else \
		cargo build --release; \
	fi
	@cp target/release/aic "$(HOME)/.local/bin/aic"
	@printf 'Built and installed %s\n' "$(HOME)/.local/bin/aic"

run-dev:
	@node scripts/dev.js

run-dev-no-autoreload:
	@LIVE_RELOAD=false node scripts/dev.js
