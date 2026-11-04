PREFIX ?= /usr/local
BINDIR = $(PREFIX)/bin
MANDIR = $(PREFIX)/share/man/man1

BIN = target/release/truetm
SRC = $(shell find src -name '*.rs') Cargo.toml Cargo.lock

all: $(BIN)

# The binary is a real target so `sudo make install` after `make` only copies
# it: a phony build target would rerun cargo as root and leave root-owned
# files in target/.
$(BIN): $(SRC)
	cargo build --release

install: $(BIN)
	install -Dm755 $(BIN) $(DESTDIR)$(BINDIR)/truetm
	install -Dm644 truetm.1 $(DESTDIR)$(MANDIR)/truetm.1

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/truetm
	rm -f $(DESTDIR)$(MANDIR)/truetm.1

clean:
	cargo clean

.PHONY: all install uninstall clean
