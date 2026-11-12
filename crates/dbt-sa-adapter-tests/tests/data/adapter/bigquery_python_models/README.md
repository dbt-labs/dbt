# BigQuery Python Models Test Project

Tests BigQuery Python models using BigFrames.

For the detailed setup, follow https://docs.getdbt.com/guides/dbt-python-bigframes?step=2

If you ran into any permission error, make sure the SA used is attached with the necessary roles

```bash
#Create Service Account
gcloud iam service-accounts create dbt-bigframes-sa
#Grant BigQuery User Role
gcloud projects add-iam-policy-binding ${GOOGLE_CLOUD_PROJECT} --member=serviceAccount:dbt-bigframes-sa@${GOOGLE_CLOUD_PROJECT}.iam.gserviceaccount.com --role=roles/bigquery.user
#Grant BigQuery Data Editor role. This can be restricted at dataset level
gcloud projects add-iam-policy-binding ${GOOGLE_CLOUD_PROJECT} --member=serviceAccount:dbt-bigframes-sa@${GOOGLE_CLOUD_PROJECT}.iam.gserviceaccount.com --role=roles/bigquery.dataEditor
#Grant Service Account user 
gcloud projects add-iam-policy-binding ${GOOGLE_CLOUD_PROJECT} --member=serviceAccount:dbt-bigframes-sa@${GOOGLE_CLOUD_PROJECT}.iam.gserviceaccount.com --role=roles/iam.serviceAccountUser
#Grant Colab Entperprise User
gcloud projects add-iam-policy-binding ${GOOGLE_CLOUD_PROJECT} --member=serviceAccount:dbt-bigframes-sa@${GOOGLE_CLOUD_PROJECT}.iam.gserviceaccount.com --role=roles/aiplatform.colabEnterpriseUser
```

# dbt-core Record

```bash
DBT_RECORDER_MODE=RECORD DBT_RECORDER_FILE_PATH=./recording.json dbt run --select basic_python_model --target bigquery_adapters
```

# dbt-fusion Replay

```bash
fs run --select basic_python_model --target bigquery_adapters --dbt-replay recording.json
```