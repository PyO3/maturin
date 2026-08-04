from subprocess import check_output


def main():
    output = check_output(["license-test"]).decode("utf-8").strip()
    if not output == "Hello, world!":
        raise RuntimeError(output)
    print("SUCCESS")


if __name__ == "__main__":
    main()
