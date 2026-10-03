.PHONY: build

build:
	if [ -x /usr/bin/distrobox-enter ]; then \
		/usr/bin/distrobox-enter -n apps-env -- cargo build --release; \
	else \
		cargo build --release; \
	fi
	cp target/release/aic "$(HOME)/.local/bin/aic"
