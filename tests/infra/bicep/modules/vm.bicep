// vm.bicep
// Creates a single Ubuntu benchmark node: NIC (Accelerated Networking ON) +
// optional management public IP + the VM itself.
//
// * Accelerated Networking is mandatory for meaningful network benchmarks and
//   is hard-enabled on the NIC. The default VM SKU (D-series v5) supports it.
//   All benchmark data-plane traffic is expected to use the NIC's *private* IP.
// * The public IP (optional) is a MANAGEMENT convenience for SSH only; the NSG
//   restricts 22/TCP to the admin CIDR and never exposes the benchmark ports.
//   Set enablePublicIp=false and reach the node via Azure Bastion for a fully
//   private posture.
// * Password auth is disabled; SSH key auth only.

@description('VM name (also used as the computer/host name and as the base for NIC/PIP names).')
param name string

@description('Azure region for the VM and its NIC.')
param location string

@description('Availability zone ("1"/"2"/"3"). Empty string = no zone pinning (regional).')
param zone string = ''

@description('Resource ID of the subnet to attach the NIC to.')
param subnetId string

@description('VM size. Must support Accelerated Networking + Premium storage (e.g. Standard_D2s_v5).')
param vmSize string

@description('Admin username for the Linux VM.')
param adminUsername string

@description('SSH public key (OpenSSH format). Supplied at deploy time; never hard-coded.')
param sshPublicKey string

@description('Resource ID of a Proximity Placement Group to co-locate the VM. Empty = none.')
param ppgId string = ''

@description('Attach a Standard public IP for management SSH. Disable to use Azure Bastion instead.')
param enablePublicIp bool = true

@description('Resource tags.')
param tags object = {}

var hasZone = !empty(zone)
var hasPpg = !empty(ppgId)

resource pip 'Microsoft.Network/publicIPAddresses@2023-11-01' = if (enablePublicIp) {
  name: '${name}-pip'
  location: location
  sku: {
    name: 'Standard'
  }
  zones: hasZone ? [ zone ] : null
  properties: {
    publicIPAllocationMethod: 'Static'
    dnsSettings: {
      domainNameLabel: toLower('${name}-${take(uniqueString(resourceGroup().id, name), 6)}')
    }
  }
  tags: tags
}

resource nic 'Microsoft.Network/networkInterfaces@2023-11-01' = {
  name: '${name}-nic'
  location: location
  tags: tags
  properties: {
    enableAcceleratedNetworking: true
    ipConfigurations: [
      {
        name: 'ipconfig1'
        properties: {
          subnet: {
            id: subnetId
          }
          privateIPAllocationMethod: 'Dynamic'
          publicIPAddress: enablePublicIp ? { id: pip.id } : null
        }
      }
    ]
  }
}

resource vm 'Microsoft.Compute/virtualMachines@2024-03-01' = {
  name: name
  location: location
  zones: hasZone ? [ zone ] : null
  tags: tags
  properties: {
    hardwareProfile: {
      vmSize: vmSize
    }
    proximityPlacementGroup: hasPpg ? { id: ppgId } : null
    osProfile: {
      computerName: name
      adminUsername: adminUsername
      linuxConfiguration: {
        disablePasswordAuthentication: true
        ssh: {
          publicKeys: [
            {
              path: '/home/${adminUsername}/.ssh/authorized_keys'
              keyData: sshPublicKey
            }
          ]
        }
      }
    }
    storageProfile: {
      imageReference: {
        publisher: 'Canonical'
        offer: '0001-com-ubuntu-server-jammy'
        sku: '22_04-lts-gen2'
        version: 'latest'
      }
      osDisk: {
        createOption: 'FromImage'
        managedDisk: {
          storageAccountType: 'Premium_LRS'
        }
      }
    }
    networkProfile: {
      networkInterfaces: [
        {
          id: nic.id
        }
      ]
    }
  }
}

@description('Private IP address of the VM (data-plane address the harness targets).')
output privateIp string = nic.properties.ipConfigurations[0].properties.privateIPAddress

@description('Management public IP (empty when enablePublicIp=false).')
output publicIp string = pip.?properties.ipAddress ?? ''

@description('Management FQDN (empty when enablePublicIp=false).')
output fqdn string = pip.?properties.dnsSettings.fqdn ?? ''

@description('VM name.')
output vmName string = vm.name
