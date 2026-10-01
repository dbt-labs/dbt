# Mom's Flower Shop - dbt Project (formerly SDF Sample)

This project was the default sample project for SDF. This is a version ported to dbt. 

## Project Structure

The project contains data about:
1. **Customers** - Customer information from the mobile app
2. **Marketing campaigns** - Marketing campaign events and costs  
3. **Mobile in-app events** - User interactions within the mobile app
4. **Street addresses** - Customer address information

## dbt Project Layout

```
├── dbt_project.yml          # dbt project configuration
├── macros/
│   ├── calculate_conversion_rate.sql    # Macro for calculating conversion rates
├── models/
│   ├── staging/                          # Staging models (materialized as views)
│   │   ├── stg_customers.sql
│   │   ├── stg_inapp_events.sql
│   │   ├── stg_marketing_campaigns.sql
│   │   ├── stg_app_installs.sql
│   │   └── stg_installs_per_campaign.sql
│   ├── analytics_new/                    # Basic analytics models (7 models)
│   │   ├── agg_installs_and_campaigns.sql       # Daily install aggregations
│   │   ├── agg_installs_ranked.sql              # Campaign rankings & performance tiers
│   │   ├── daily_active_users.sql               # DAU/WAU/MAU metrics
│   │   ├── platform_performance_metrics.sql     # Platform comparison metrics
│   │   ├── hourly_event_patterns.sql            # Time-of-day usage patterns
│   │   ├── geographic_analysis.sql              # State-level analysis
│   │   └── campaign_roi_dashboard.sql           # Executive ROI dashboard
│   └── analytics/                        # Advanced analytics models (14 models)
│       ├── campaign_performance_summary.sql     # Campaign ROI & conversion metrics
│       ├── campaign_comparison.sql              # Campaign benchmarking & retention
│       ├── customer_acquisition_cost.sql        # CAC & LTV analysis
│       ├── customer_lifetime_value.sql          # Customer LTV calculations
│       ├── customer_cohort_retention.sql        # Cohort retention analysis
│       ├── customer_segmentation.sql            # RFM segmentation
│       ├── customer_journey_time.sql            # Time-to-conversion analysis
│       ├── customer_360_view.sql                # Comprehensive customer view
│       ├── event_funnel_analysis.sql            # Conversion funnel tracking
│       ├── weekly_growth_metrics.sql            # Week-over-week growth
│       ├── monthly_revenue_trends.sql           # Monthly revenue & trends
│       ├── churn_risk_analysis.sql              # Churn prediction & scoring
│       ├── product_affinity_analysis.sql        # Event sequences & transitions
│       ├── user_engagement_score.sql            # Engagement scoring (incremental)
│       ├── session_analysis.sql                 # Session duration & patterns
│       ├── marketing_channel_attribution.sql    # Attribution modeling
│       ├── repeat_purchase_analysis.sql         # Purchase behavior patterns
│       ├── executive_kpi_summary.sql            # Daily KPIs (ephemeral)
│       ├── rolling_metrics_snapshot.sql         # Rolling 7/30-day metrics
│       ├── high_value_customers_audit.sql       # High-value customers (audit_table)
│       └── daily_revenue_summary_audit.sql      # Daily revenue (audit_table)
├── seeds/                   # CSV seed files
│   ├── raw_customers.csv
│   ├── raw_addresses.csv
│   ├── raw_inapp_events.csv
│   └── raw_marketing_campaign_events.csv
└── tests/                   # Data quality tests (defined in schema.yml files)
```

## Database Configuration

This project uses the `internal analytics` (KW277..) profile with the following configuration:
- **Database**: RAW
- **Warehouse**: TRANSFORMING  
- **Schema**: moms_flower_shop_<your-name>
- **Role**: TRANSFORMER

__For deferring, use the production schema `moms_flower_shop`. This can be helpful for the compare changes demo__

## Getting Started

After updating your schema:

1. **Load seed data and build**:
   ```bash
   dbt build
   ```

## Analytics Models Organization

The analytics layer is split into two directories based on complexity:

### `analytics_new/` - Basic Analytics (7 models)
Foundational, straightforward analytics models with simple aggregations:
- **Daily & Time-based Metrics**: Install aggregations, DAU/WAU/MAU, hourly patterns
- **Performance Basics**: Campaign rankings, platform metrics, geographic analysis
- **Executive Views**: Simple dashboard combining key metrics

These models use basic CTEs and aggregations, perfect for getting started or learning dbt.

### `analytics/` - Advanced Analytics (14 models)
Complex analytical models with sophisticated business logic:
- **Customer Analytics**: LTV, cohort retention, segmentation (RFM), 360° view, journey timing
- **Campaign Analytics**: Performance summary, comparison, ROI, acquisition cost, attribution
- **Behavioral Analytics**: Event funnels, session analysis, product affinity, engagement scoring
- **Revenue Analytics**: Weekly/monthly trends, repeat purchase analysis, churn risk
- **Special Features**: 
  - Incremental materialization (`user_engagement_score`)
  - Ephemeral models (`executive_kpi_summary`)

## Model Lineage

```
Seeds (CSV files)
    ↓  
Staging Models (Views with stg_ prefix)
    ↓
Analytics 
```

## Troubleshooting

If you encounter issues:

1. **Profile not found**: Ensure the `ia_dev` profile exists in `~/.dbt/profiles.yml`
2. **Permission errors**: Verify the TRANSFORMER role has appropriate permissions
3. **Seed loading issues**: Check CSV file formatting and column names match model expectations
4. **Model compilation errors**: Review SQL syntax and ensure all referenced models exist

For more information, see the [dbt documentation](https://docs.getdbt.com/).

