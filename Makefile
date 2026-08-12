.PHONY: all clean step

-include .env
export

all:
	cargo run --manifest-path drawio-lc/Cargo.toml -- test.yaml 

step:
	cargo run --manifest-path drawio-lc/Cargo.toml -- test.yaml --step $(step)


clean:
	rm -f *.png *.gif *.mp4 *.html step*.drawio
