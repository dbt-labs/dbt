<!-- GitHub will publish this readme on the main repo page if the name is `README.md` so we've added the leading underscore to prevent this -->
<!-- Do not rename this file `README.md` -->
<!-- See https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-readmes -->

## What are GitHub Actions?

GitHub Actions are used for many different purposes.  We use them to run tests in CI, validate PRs are in an expected state, and automate processes.

- [Overview of GitHub Actions](https://docs.github.com/en/actions/learn-github-actions/understanding-github-actions)
- [What's a workflow?](https://docs.github.com/en/actions/using-workflows/about-workflows)
- [GitHub Actions guides](https://docs.github.com/en/actions/guides)

___

## Where do actions and workflows live

We try to maintain actions that are shared across repositories in a single place so that necesary changes can be made in a single place.

[dbt-labs/actions](https://github.com/dbt-labs/actions/) is the central repository of actions and workflows we use across repositories.

GitHub Actions also live locally within a repository.  The workflows can be found at `.github/workflows` from the root of the repository.  These should be specific to that code base.

Note: We are actively moving actions into the central Action repository so there is currently some duplication across repositories.

___

## Basics of Using Actions

### Viewing Output

- View the detailed action output for your PR in the **Checks** tab of the PR.  This only shows the most recent run.  You can also view high level **Checks** output at the bottom on the PR.

- View _all_ action output for a repository from the [**Actions**](https://github.com/dbt-labs/dbt/actions) tab.  Workflow results last 1 year.  Artifacts last 90 days, unless specified otherwise in individual workflows.

  This view often shows what seem like duplicates of the same workflow.  This occurs when files are renamed but the workflow name has not changed.  These are in fact _not_ duplicates.

  You can see the branch the workflow runs from in this view.  It is listed in the table between the workflow name and the time/duration of the run.  When blank, the workflow is running in the context of the  `main` branch.

### How to view what workflow file is being referenced from a run

- When viewing the output of a specific workflow run, click the 3 dots at the top right of the display.  There will be an option to `View workflow file`.

### How to manually run a workflow

- If a workflow has the `on: workflow_dispatch` trigger, it can be manually triggered
- From the [**Actions**](https://github.com/dbt-labs/dbt/actions) tab, find the workflow you want to run, select it and fill in any inputs requied.  That's it!

### How to re-run jobs

- From the UI you can rerun from failure
- You can retrigger the cla check by commenting on the PR with `@cla-bot check`

___

## General Standards

### Permissions
- By default, workflows have read permissions in the repository for the contents scope only when no permissions are explicitly set.
- It is best practice to always define the permissions explicitly.  This will allow actions to continue to work when the default permissions on the repository are changed.  It also allows explicit grants of the least permissions possible.
- There are a lot of permissions available.  [Read up on them](https://docs.github.com/en/actions/using-jobs/assigning-permissions-to-jobs) if you're unsure what to use.

```yaml
permissions:
  contents: read
  pull-requests: write
```

### Secrets
- When to use a [Personal Access Token (PAT)](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/creating-a-personal-access-token) vs the [GITHUB_TOKEN](https://docs.github.com/en/actions/security-guides/automatic-token-authentication) generated for the action?

    The `GITHUB_TOKEN` is used by default.  In most cases it is sufficient for what you need.

    If you expect the workflow to result in a commit to that should retrigger workflows, you will need to use a Personal Access Token for the bot to commit the file. When using the GITHUB_TOKEN, the resulting commit will not trigger another GitHub Actions Workflow run. This is due to limitations set by GitHub. See [the docs](https://docs.github.com/en/actions/security-guides/automatic-token-authentication#using-the-github_token-in-a-workflow) for a more detailed explanation.

    For example, we must use a PAT in our workflow to commit a new changelog yaml file for bot PRs.  Once the file has been committed to the branch, it should retrigger the check to validate that a changelog exists on the PR.  Otherwise, it would stay in a failed state since the check would never retrigger.

### Triggers
You can configure your workflows to run when specific activity on GitHub happens, at a scheduled time, or when an event outside of GitHub occurs.  Read more details in the [GitHub docs](https://docs.github.com/en/actions/using-workflows/events-that-trigger-workflows).

These triggers are under the `on` key of the workflow and more than one can be listed.

```yaml
on:
  push:
    branches:
      - "main"
      - "*.latest"
      - "releases/*"
  pull_request:
    # catch when the PR is opened with the label or when the label is added
    types: [opened, labeled]
  workflow_dispatch:
```

Some triggers of note that we use:

- `push` - Runs your workflow when you push a commit or tag.
- `pull_request` - Runs your workflow when activity on a pull request in the workflow's repository occurs.  Takes in a list of activity types (opened, labeled, etc) if appropriate.
- `pull_request_target` - Same as `pull_request` but runs in the context of the PR target branch.
- `workflow_call` - used with reusable workflows.  Triggered by another workflow calling it.
- `workflow_dispatch` - Gives the ability to manually trigger a workflow from the GitHub API, GitHub CLI, or GitHub browser interface.


### Basic Formatting
- Add a description of what your workflow does at the top in this format

  ```
  # **what?**
  # Describe what the action does.

  # **why?**
  # Why does this action exist?

  # **when?**
  # How/when will it be triggered?
  ```

- Leave blank lines between steps and jobs

  ```yaml
  jobs:
    dependency_changelog:
      runs-on: ${{ vars.UBUNTU_LATEST }}

      steps:
      - name: Get File Name Timestamp
        id: filename_time
        uses: nanzm/get-time-action@v1.1
        with:
          format: 'YYYYMMDD-HHmmss'

      - name: Get File Content Timestamp
        id: file_content_time
        uses: nanzm/get-time-action@v1.1
        with:
          format: 'YYYY-MM-DDTHH:mm:ss.000000-05:00'

      - name: Generate Filepath
        id: fp
        run: |
          FILEPATH=.changes/unreleased/Dependencies-${{ steps.filename_time.outputs.time }}.yaml
          echo "FILEPATH=$FILEPATH" >> $GITHUB_OUTPUT
  ```

- Print out all variables you will reference as the first step of a job.  This allows for easier debugging.  The first job should log all inputs.  Subsequent jobs should reference outputs of other jobs, if present.

  When possible, generate variables at the top of your workflow in a single place to reference later.  This is not always strictly possible since you may generate a value to be used later mid-workflow.

  Be sure to use quotes around these logs so special characters are not interpreted.

  ```yaml
  job1:
    - name: "[DEBUG] Print Variables"
      run: |
        echo "all variables defined as inputs"
        echo "The last commit sha in the release: ${{ inputs.sha }}"
        echo "The release version number:         ${{ inputs.version_number }}"
        echo "The changelog_path:                 ${{ inputs.changelog_path }}"
        echo "The build_script_path:              ${{ inputs.build_script_path }}"
        echo "The s3_bucket_name:                 ${{ inputs.s3_bucket_name }}"
        echo "The package_test_command:           ${{ inputs.package_test_command }}"

    # collect all the variables that need to be used in subsequent jobs
    - name: Set Variables
      id: variables
      run: |
        echo "important_path='performance/runner/Cargo.toml'" >> $GITHUB_OUTPUT
        echo "release_id=${{github.event.inputs.release_id}}" >> $GITHUB_OUTPUT
        echo "open_prs=${{github.event.inputs.open_prs}}" >> $GITHUB_OUTPUT

  job2:
    needs: [job1]
      - name: "[DEBUG] Print Variables"
      run: |
        echo "all variables defined in job1 > Set Variables > outputs"
        echo "important_path: ${{ needs.job1.outputs.important_path }}"
        echo "release_id:     ${{ needs.job1.outputs.release_id }}"
        echo "open_prs:       ${{ needs.job1.outputs.open_prs }}"
  ```

- When it's not obvious what something does, add a comment!

___

## Tips

### Context
- The [GitHub CLI](https://cli.github.com/) is available in the default runners
- Actions run in your context.  ie, using an action from the marketplace that uses the GITHUB_TOKEN uses the GITHUB_TOKEN generated by your workflow run.

### Runners
- We dynamically set runners based on repository vars.  Admins can view repository vars and reset them.  Current values are the following but are subject to change:
  - `vars.UBUNTU_LATEST` -> `ubuntu-latest`
  - `vars.WINDOWS_LATEST` -> `windows-latest`
  - `vars.MACOS_LATEST` -> `macos-14`

### Actions from the Marketplace
- Don’t use external actions for things that can easily be accomplished manually.
- Always read through what an external action does before using it!  Often an action in the GitHub Actions Marketplace can be replaced with a few lines in bash.  This is much more maintainable (and won’t change under us) and clear as to what’s actually happening.  It also prevents any
- Pin actions _we don't control_ to tags.

### Connecting to AWS
- Authenticate with the aws managed workflow

  ```yaml
  - name: Configure AWS credentials from Test account
    uses: aws-actions/configure-aws-credentials@v2
    with:
      aws-access-key-id: ${{ secrets.AWS_ACCESS_KEY_ID }}
      aws-secret-access-key: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
      aws-region: us-east-1
  ```

- Then access with the aws command that comes installed on the action runner machines

  ```yaml
  - name: Copy Artifacts from S3 via CLI
    run: aws s3 cp ${{ env.s3_bucket }} . --recursive
  ```

### Testing

- Depending on what your action does, you may be able to use [`act`](https://github.com/nektos/act) to test the action locally.  Some features of GitHub Actions do not work with `act`, among those are reusable workflows.  If you can't use `act`, you'll have to push your changes up before being able to test.  This can be slow.
