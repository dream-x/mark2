"""Print the pending review queue."""

PENDING = [
    ("ORB-118", "cache: drop stale entries on reload"),
    ("ORB-121", "api: paginate /reviews"),
]


def main():
    for key, title in PENDING:
        print(f"{key}  {title}")


if __name__ == "__main__":
    main()
