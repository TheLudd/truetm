PREFIX ?= /usr/local
BINDIR = $(PREFIX)/bin
MANDIR = $(PREFIX)/share/man/man1

all: build

build:
	cargo build --release

install: build
	install -Dm755 target/release/dvtr $(DESTDIR)$(BINDIR)/dvtr
	install -Dm644 dvtr.1 $(DESTDIR)$(MANDIR)/dvtr.1

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/dvtr
	rm -f $(DESTDIR)$(MANDIR)/dvtr.1

clean:
	cargo clean

.PHONY: all build install uninstall clean
