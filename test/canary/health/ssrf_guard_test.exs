defmodule Canary.Health.SSRFGuardTest do
  use ExUnit.Case, async: true

  alias Canary.Health.SSRFGuard

  describe "validate_url/2" do
    test "allows valid HTTPS URLs" do
      assert :ok = SSRFGuard.validate_url("https://example.com/health")
    end

    test "allows valid HTTP URLs" do
      assert :ok = SSRFGuard.validate_url("http://example.com/health")
    end

    test "rejects non-HTTP schemes" do
      assert {:error, msg} = SSRFGuard.validate_url("ftp://example.com")
      assert msg =~ "scheme"
    end

    test "rejects missing host" do
      assert {:error, _} = SSRFGuard.validate_url("https://")
    end

    test "blocks loopback IPs" do
      assert {:error, msg} = SSRFGuard.validate_url("http://127.0.0.1/health")
      assert msg =~ "blocked"
    end

    test "blocks private IPs (10.x)" do
      assert {:error, _} = SSRFGuard.validate_url("http://10.0.0.1/health")
    end

    test "blocks private IPs (192.168.x)" do
      assert {:error, _} = SSRFGuard.validate_url("http://192.168.1.1/health")
    end

    test "blocks link-local (169.254.x)" do
      assert {:error, _} = SSRFGuard.validate_url("http://169.254.169.254/latest/meta-data")
    end

    test "allows private IPs when allow_private is true" do
      assert :ok = SSRFGuard.validate_url("http://10.0.0.1/health", true)
    end
  end
end
