from subprocess import check_output


def main():
    output = check_output(["hello-world"]).decode("utf-8").strip()
    if not output == "Hello, world!":
        raise Exception(output)

    output = check_output(["foo"]).decode("utf-8").strip()
    if not output == "🦀 Hello, world! 🦀":
        raise Exception(output)
    print("SUCCESS")


if __name__ == "__main__":
    main()
