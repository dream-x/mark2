The file `deploy.yaml` in the current working directory is a deployment
manifest with several environments and services.

Set `replicas` of the `api` service in the `prod` environment to 6.

Rules:

- The file is compared byte-for-byte with the original afterwards: exactly one
  line may differ.
- Do not reformat the file: keep the quoting, the indentation, the key order
  and the blank lines as they are.
- Do not touch any comments.
- Do not change the `staging` environment or any other service.
- Do not create backups or any other new files.
